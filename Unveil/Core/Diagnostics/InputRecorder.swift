//
//  InputRecorder.swift
//  Unveil
//

import Foundation
import QuartzCore
import Synchronization
import UIKit

#if DEBUG

/// InputRecorder is the Debug-only recorder of a real trackpad session (`-UnveilRecordInput <s>`),
/// built because the agent cannot pinch: someone zooms by hand while it writes, and the CSV says
/// whether the lag is in the events, in the main thread or in the frames.
///
/// It keeps one row per canvas input event (pinch, two-finger scroll through the scroll proxy,
/// Cmd + scroll): when it was handled, the recognizer state, the raw scale or translation, the
/// factor the zoom was given and the zoom it reached, whether it was indirect (no touches), and
/// how long the handler held main. It keeps one row per display link update of the canvas: the
/// update's time and target presentation time, the main time of the update (the draw included)
/// and of the gesture handlers since the previous update, whether it drew, whether the window the
/// canvas asked for had not landed yet (a window render in flight), and, once Metal reports it,
/// when its drawable was presented (0: never shown). Frame rows also carry the scroll proxy's
/// offset and the image offset at that update, and whether the proxy glides (fingers lifted).
///
/// Times are media-time seconds from the start (`CACurrentMediaTime`, the presented time's clock
/// too). It stops by itself after the given time, writes `Documents/measurements/<time>.input.csv`
/// and prints its path. A Release build has the stub below: nothing kept or written.
nonisolated final class InputRecorder: Sendable {

    static let shared = InputRecorder()

    static let header = "kind,t,source,state,raw_x,raw_y,factor,zoom,indirect,handler_ms,"
        + "target_t,update_ms,gesture_ms,drew,window_in_flight,drawable,presented_t,"
        + "proxy_x,proxy_y,offset_x,offset_y,gliding"

    private let state = Mutex(State())

    private struct Row {

        var kind           : String
        var time           : CFTimeInterval
        var source         = ""
        var gestureState   = ""
        var rawX           = 0.0
        var rawY           = 0.0
        var factor         = 0.0
        var zoom           = 0.0
        var isIndirect     = false
        var handlerTime    = 0.0
        var targetTime     = 0.0
        var updateTime     = 0.0
        var gestureTime    = 0.0
        var drew           = false
        var windowInFlight = false
        var drawable       = 0
        var presentedTime  = -1.0
        var proxyX         = 0.0
        var proxyY         = 0.0
        var offsetX        = 0.0
        var offsetY        = 0.0
        var isGliding      = false
    }

    private struct State {

        var isRecording: Bool = false
        var start      : CFTimeInterval = 0
        var rows       : [Row] = []
        var gestureTime: Double = 0

        /// drawableRows maps a drawn drawable's id to its update row, until it is presented.
        var drawableRows: [Int: Int] = [:]
    }

    /// now is the clock a handler reads on entry and hands to `event`, 0 while not recording.
    var now: CFTimeInterval {
        state.withLock { $0.isRecording } ? CACurrentMediaTime() : 0
    }

    /// record starts a recording, then stops it after `seconds` and writes the CSV. Main only.
    @MainActor
    func record(for seconds: Double) async {
        state.withLock { current in
            current = State()
            current.isRecording = true
            current.start       = CACurrentMediaTime()
        }

        print("input recording for \(seconds) s")
        try? await Task.sleep(for: .seconds(seconds))

        let rows = state.withLock { current in
            current.isRecording = false
            return current.rows
        }

        let stamp   = ISO8601DateFormatter().string(from: .now).replacingOccurrences(of: ":", with: "")
        let fileURL = URL.documentsDirectory.appending(path: "measurements/\(stamp).input.csv")
        let text    = ([Self.header] + rows.map(Self.line)).joined(separator: "\n") + "\n"

        do {
            try FileManager.default.createDirectory(
                at                          : fileURL.deletingLastPathComponent(),
                withIntermediateDirectories : true
            )
            try Data(text.utf8).write(to: fileURL)
            print("input csv \(fileURL.path(percentEncoded: false)) (\(rows.count) rows)")
        } catch {
            print("input csv not written: \(error.localizedDescription)")
        }
    }

    /// event records one input event handled since `began` (from `now`). `rawX` and `rawY` are the
    /// translation of a scroll, or the scale in `rawX` for a pinch. Main only.
    func event(
        _ source   : String,
        state      : UIGestureRecognizer.State,
        rawX       : Double,
        rawY       : Double = 0,
        factor     : Double,
        zoom       : Double,
        isIndirect : Bool,
        began      : CFTimeInterval
    ) {
        guard began > 0 else { return }

        let end = CACurrentMediaTime()

        self.state.withLock { current in
            guard current.isRecording else { return }

            current.gestureTime += end - began
            current.rows.append(Row(
                kind         : "event",
                time         : end - current.start,
                source       : source,
                gestureState : Self.name(of: state),
                rawX         : rawX,
                rawY         : rawY,
                factor       : factor,
                zoom         : zoom,
                isIndirect   : isIndirect,
                handlerTime  : end - began
            ))
        }
    }

    /// update records one display link update of the canvas that began at `began`. `drawable` is
    /// the id of the drawable it drew into, nil when it did not draw. `proxy` is the scroll proxy's
    /// offset and `offset` the image offset after the update. Main only.
    func update(
        began          : CFTimeInterval,
        target         : CFTimeInterval,
        zoom           : Double,
        drawable       : Int?,
        windowInFlight : Bool,
        proxy          : CGPoint,
        offset         : SIMD2<Double>,
        isGliding      : Bool
    ) {
        guard began > 0 else { return }

        let end = CACurrentMediaTime()

        state.withLock { current in
            guard current.isRecording else { return }

            if let drawable {
                current.drawableRows[drawable] = current.rows.count
            }

            current.rows.append(Row(
                kind           : "frame",
                time           : began - current.start,
                zoom           : zoom,
                targetTime     : target - current.start,
                updateTime     : end - began,
                gestureTime    : current.gestureTime,
                drew           : drawable != nil,
                windowInFlight : windowInFlight,
                drawable       : drawable ?? 0,
                proxyX         : Double(proxy.x),
                proxyY         : Double(proxy.y),
                offsetX        : offset.x,
                offsetY        : offset.y,
                isGliding      : isGliding
            ))
            current.gestureTime = 0
        }
    }

    /// encoded attaches an asynchronous draw to the tick that requested it. Only encoding time
    /// counts as main work; the off-main drawable wait is excluded from updateTime.
    func encoded(
        drawable    : Int,
        updateBegan : CFTimeInterval,
        encodeBegan : CFTimeInterval,
        zoom        : Double,
        offset      : SIMD2<Double>
    ) {
        guard updateBegan > 0, encodeBegan > 0 else { return }

        let end = CACurrentMediaTime()
        state.withLock { current in
            guard current.isRecording,
                  let row = current.rows.lastIndex(where: {
                      $0.kind == "frame" && $0.time == updateBegan - current.start
                  })
            else { return }

            current.drawableRows[drawable] = row
            current.rows[row].drawable = drawable
            current.rows[row].drew = true
            current.rows[row].zoom = zoom
            current.rows[row].offsetX = offset.x
            current.rows[row].offsetY = offset.y
            current.rows[row].updateTime += end - encodeBegan
        }
    }

    /// presented fills in when the drawable `drawable` was presented (0: never shown). Any thread.
    func presented(drawable: Int, at time: CFTimeInterval) {
        state.withLock { current in
            guard current.isRecording, let row = current.drawableRows.removeValue(forKey: drawable) else { return }

            current.rows[row].presentedTime = time > 0 ? time - current.start : 0
        }
    }

    private static func line(_ row: Row) -> String {
        String(
            format: "%@,%.6f,%@,%@,%.4f,%.4f,%.6f,%.6f,%d,%.3f,%.6f,%.3f,%.3f,%d,%d,%d,%.6f,%.3f,%.3f,%.3f,%.3f,%d",
            row.kind,
            row.time,
            row.source,
            row.gestureState,
            row.rawX,
            row.rawY,
            row.factor,
            row.zoom,
            row.isIndirect ? 1 : 0,
            row.handlerTime * 1000,
            row.targetTime,
            row.updateTime * 1000,
            row.gestureTime * 1000,
            row.drew ? 1 : 0,
            row.windowInFlight ? 1 : 0,
            row.drawable,
            row.presentedTime,
            row.proxyX,
            row.proxyY,
            row.offsetX,
            row.offsetY,
            row.isGliding ? 1 : 0
        )
    }

    private static func name(of state: UIGestureRecognizer.State) -> String {
        switch state {
            case .began    : "began"
            case .changed  : "changed"
            case .ended    : "ended"
            case .cancelled: "cancelled"
            case .failed   : "failed"
            default        : "possible"
        }
    }
}

#else

/// InputRecorder in Release keeps only the entry points, empty and inlined: nothing is recorded.
nonisolated final class InputRecorder: Sendable {

    static let shared = InputRecorder()

    @inline(__always)
    var now: CFTimeInterval { 0 }

    @inline(__always)
    func record(for seconds: Double) async {}

    @inline(__always)
    func event(
        _ source   : String,
        state      : UIGestureRecognizer.State,
        rawX       : Double,
        rawY       : Double = 0,
        factor     : Double,
        zoom       : Double,
        isIndirect : Bool,
        began      : CFTimeInterval
    ) {}

    @inline(__always)
    func update(
        began          : CFTimeInterval,
        target         : CFTimeInterval,
        zoom           : Double,
        drawable       : Int?,
        windowInFlight : Bool,
        proxy          : CGPoint,
        offset         : SIMD2<Double>,
        isGliding      : Bool
    ) {}

    @inline(__always)
    func presented(drawable: Int, at time: CFTimeInterval) {}

    @inline(__always)
    func encoded(
        drawable    : Int,
        updateBegan : CFTimeInterval,
        encodeBegan : CFTimeInterval,
        zoom        : Double,
        offset      : SIMD2<Double>
    ) {}
}

#endif
