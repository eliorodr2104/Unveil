//
//  StressSweep.swift
//  Unveil
//

import QuartzCore
import UIKit

/// StressSweep drives the exposure slider by itself for a fixed time, the soak test of T15: on
/// every tick of a 60 Hz CADisplayLink it drags `light.exposure` along a sine between -2 and +2 with
/// a 4 s period, and once a second it releases, which asks for a full render. It goes through
/// `EditorViewModel.drag` and `endDrag`, the same coalesced path as a finger, so its
/// `SliderToFrame` samples are the ones a user would get.
///
/// While it runs, FootprintSampler writes the memory CSV and the idle timer is off, so the screen
/// stays on and the display link keeps firing. It stops at the end of `duration`, or early when
/// the app resigns active (a backgrounded app gets no renders), and tells `onFinish` which one
/// happened and where the CSV is. The display link retains the sweep while it runs, so the caller
/// does not need to keep it.
final class StressSweep<Engine: EngineDriving, Importer: PhotoImporting>: NSObject {

    nonisolated static var amplitude    : Double { 2 }
    nonisolated static var periodSeconds: Double { 4 }

    private let viewModel: EditorViewModel<Engine, Importer>
    private let duration : Double
    private let sampler  = FootprintSampler()
    private let onFinish : @MainActor (_ isComplete: Bool, _ csv: URL) -> Void

    private var displayLink: CADisplayLink?
    private var startTime  : CFTimeInterval = 0
    private var lastElapsed: Double         = 0

    // The notification token type is an existential declared by Foundation.
    private var resignToken: (any NSObjectProtocol)?

    init(
        viewModel: EditorViewModel<Engine, Importer>,
        duration : Double,
        onFinish : @escaping @MainActor (_ isComplete: Bool, _ csv: URL) -> Void
    ) {
        self.viewModel = viewModel
        self.duration  = duration
        self.onFinish  = onFinish
    }

    /// exposure is the dragged value `elapsed` seconds into the sweep.
    nonisolated static func exposure(at elapsed: Double) -> Double {
        amplitude * sin(2 * Double.pi * elapsed / periodSeconds)
    }

    /// isRelease tells whether a whole second boundary lies in (previous, current]: the tick that
    /// crosses it releases. Ticks are about 16 ms apart, so each second releases exactly once.
    nonisolated static func isRelease(from previous: Double, to current: Double) -> Bool {
        current.rounded(.down) > previous.rounded(.down)
    }

    func start() {
        guard displayLink == nil else { return }

        sampler.start()
        UIApplication.shared.isIdleTimerDisabled = true

        resignToken = NotificationCenter.default.addObserver(
            forName : UIApplication.willResignActiveNotification,
            object  : nil,
            queue   : .main
        ) { [weak self] _ in
            Task { @MainActor in self?.finish(isComplete: false) }
        }

        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 60, preferred: 60)
        link.add(to: .main, forMode: .common)

        startTime   = CACurrentMediaTime()
        displayLink = link

        print("sweep started: \(duration) s")
    }

    @objc
    private func tick(_ link: CADisplayLink) {
        let elapsed = link.timestamp - startTime

        guard elapsed < duration else {
            finish(isComplete: true)
            return
        }

        let value = Self.exposure(at: elapsed)
        Task { await viewModel.drag(.exposure, to: value) }

        if Self.isRelease(from: lastElapsed, to: elapsed) {
            Task { await viewModel.endDrag(.exposure) }
        }

        lastElapsed = elapsed
    }

    /// finish runs once: the first stop (time up or resign) wins, a later one finds no link.
    private func finish(isComplete: Bool) {
        guard let link = displayLink else { return }

        link.invalidate()
        displayLink = nil

        if let resignToken {
            NotificationCenter.default.removeObserver(resignToken)
        }

        resignToken = nil
        UIApplication.shared.isIdleTimerDisabled = false

        Task {
            await viewModel.endDrag(.exposure)
            await sampler.stop()

            let outcome = isComplete ? "completed" : "stopped early"
            print("sweep \(outcome): \(sampler.fileURL.path(percentEncoded: false))")
            onFinish(isComplete, sampler.fileURL)
        }
    }
}
