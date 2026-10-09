//
//  Signposts.swift
//  Unveil
//

import os
import Synchronization

/// Signposts is the one OSSignposter of the measurements (subsystem `com.unveil`, category `Engine`)
/// and the intervals that need more than a begin and an end in the same scope.
///
/// - `OpenToFirstFrame`: from `EditorViewModel.open` to the presented frame at or above the open's
///   preview generation. It ends with `failed` when the open never asked for a preview.
/// - `SliderToFrame` (`draft` or `full`): from `EngineManager.requestPreview`, the single place a
///   preview is asked for, to the presented frame at or above its generation. It ends with
///   `presented` for its own frame, `dropped` for its own frame with no presented time, and
///   `overtaken` when a newer frame was shown instead.
/// - `Command` (the command name), `ImportCopy` and `ShaderCompile` are begun and ended in place by
///   EngineManager, EditorViewModel and CanvasShaderSource through `signposter`.
///
/// "Presented" means the canvas's drawable left the display pipeline (`addPresentedHandler`, on a
/// Metal thread), not that the engine delivered the pixels: what the user sees is what is measured.
/// A drawable Metal reports with no presented time ends its own interval as `dropped`, so a trace
/// can tell those apart (or leave them out) instead of losing the interval.
///
/// The signposts and their bookkeeping always run, Instruments attached or not: the `Engine`
/// category is an ordinary log, so `isEnabled` is true and each event goes to the in-memory log.
/// The cost is a few microseconds per event (one Mutex lock and a small array per request and per
/// present) plus one presented-handler closure per drawn frame. The upside is that the matching
/// never has a gap when xctrace attaches mid-run. The `isEnabled` guard only matters if the log is
/// disabled by configuration.
///
/// Every preview request begins a `SliderToFrame`, the open's full request and `refreshPreview`
/// included, so a sweep's statistics should keep only the intervals begun after the sweep started.
nonisolated enum Signposts {

    static let signposter = OSSignposter(subsystem: "com.unveil", category: "Engine")

    private static let waiting = Mutex(PendingFrameIntervals<WaitingInterval>())

    private enum WaitingInterval: Sendable {

        case sliderToFrame(OSSignpostIntervalState)
        case openToFirstFrame(OSSignpostIntervalState)
    }

    /// previewRequested begins one `SliderToFrame` for the render that will carry `generation`.
    static func previewRequested(generation: UInt64, isDraft: Bool) {
        guard signposter.isEnabled else { return }

        let quality = isDraft ? "draft" : "full"
        let state   = signposter.beginInterval(
            "SliderToFrame",
            id: signposter.makeSignpostID(),
            "\(quality, privacy: .public)"
        )

        wait(.sliderToFrame(state), for: generation)
    }

    /// beginOpen starts `OpenToFirstFrame`; hand the state to `openRequestedPreview` once the open
    /// knows its preview generation, or failed.
    static func beginOpen() -> OSSignpostIntervalState {
        signposter.beginInterval("OpenToFirstFrame", id: signposter.makeSignpostID())
    }

    /// openRequestedPreview makes the open wait for `generation`, or ends it as `failed` when nil.
    static func openRequestedPreview(_ state: OSSignpostIntervalState, generation: UInt64?) {
        guard let generation else {
            signposter.endInterval("OpenToFirstFrame", state, "failed")
            return
        }

        wait(.openToFirstFrame(state), for: generation)
    }

    /// framePresented ends every interval waiting for `generation` or less. Any thread.
    static func framePresented(generation: UInt64, isDropped: Bool) {
        let ended = waiting.withLock { $0.presented(generation) }

        for entry in ended {
            if entry.generation < generation {
                end(entry.token, outcome: "overtaken")
            } else if isDropped {
                end(entry.token, outcome: "dropped")
            } else {
                end(entry.token, outcome: "presented")
            }
        }
    }

    private static func wait(_ interval: WaitingInterval, for generation: UInt64) {
        let isWaiting = waiting.withLock { $0.add(interval, waitingFor: generation) }

        if !isWaiting {
            end(interval, outcome: "presented")
        }
    }

    private static func end(_ interval: WaitingInterval, outcome: String) {
        switch interval {
            case .sliderToFrame(let state):
                signposter.endInterval("SliderToFrame", state, "\(outcome, privacy: .public)")

            case .openToFirstFrame(let state):
                signposter.endInterval("OpenToFirstFrame", state, "\(outcome, privacy: .public)")
        }
    }
}
