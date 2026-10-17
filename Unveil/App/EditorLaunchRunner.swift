//
//  EditorLaunchRunner.swift
//  Unveil
//

import UIKit

/// EditorLaunchRunner preserves the diagnostic launch arguments independently of the app's
/// scene framework. Each editor starts these commands once, even if its root view reappears.
@MainActor
final class EditorLaunchRunner {

    private var hasStarted: Bool = false

    func start(
        engine : EngineManager,
        session: EditorSession<EngineManager, PhotoImporter, PhotoLibraryLoader>,
        canvas : CanvasView,
        options: MeasurementLaunchOptions
    ) {
        guard !hasStarted else { return }

        hasStarted = true
        ProcessCostProbe.registerMainThread()
        runLaunchCommand(engine: engine)
        #if DEBUG
        runInputRecording(options: options, engine: engine)
        #endif
        runMeasurement(
            options : options,
            session : session,
            canvas  : canvas,
            engine  : engine
        )
    }

    /// runLaunchCommand runs the diagnostic named by the `-UnveilRun` launch argument, once per
    /// launch, so an agent can drive the device with no taps (`devicectl device process launch`).
    ///
    /// `golden` exports the reference images, prints its progress and ends the process: `exit(0)` when
    /// every image was written, `exit(1)` otherwise. The exit is allowed only in this agent-driven
    /// mode, where nobody is editing. `state` appends one `engine.jsonl` line and stays open.
    private func runLaunchCommand(engine: EngineManager) {
        guard let command = UserDefaults.standard.string(forKey: "UnveilRun") else { return }

        Task {
            // The engine is suspended while the app is not active: wait for the first activation.
            while UIApplication.shared.applicationState != .active {
                try? await Task.sleep(for: .milliseconds(100))
            }

            switch command {
                case "golden":
                    let summary = await GoldenExporter(engine: engine).run()
                    exit(summary.failures.isEmpty ? 0 : 1)

                case "state":
                    engine.recordState(event: "run-state")

                case "disk":
                    Self.printDiskSpace()
                    exit(0)

                case "metal":
                    await Self.calibrateMemory(frames: engine.frames)
                    exit(0)

                default:
                    print("unknown -UnveilRun value: \(command)")
            }
        }
    }

    /// printDiskSpace prints the volume's free bytes, as iPadOS reports them to apps, so an agent can
    /// see what a device run costs in storage (`-UnveilRun disk`, then the process ends).
    private static func printDiskSpace() {
        let keys: Set<URLResourceKey> = [
            .volumeTotalCapacityKey,
            .volumeAvailableCapacityKey,
            .volumeAvailableCapacityForImportantUsageKey,
        ]

        do {
            let values = try URL.homeDirectory.resourceValues(forKeys: keys)
            print("disk: total \(values.volumeTotalCapacity ?? -1)"
                + " available \(values.volumeAvailableCapacity ?? -1)"
                + " important \(values.volumeAvailableCapacityForImportantUsage ?? -1)")
        } catch {
            print("disk: unreadable: \(error.localizedDescription)")
        }
    }

    #if DEBUG
    /// runInputRecording records the canvas's input for `-UnveilRecordInput` seconds (a Debug build,
    /// see InputRecorder), from the first full frame of `-UnveilOpen` or else from the first
    /// activation, and ends the process afterwards with `-UnveilExitAfterSweep`.
    private func runInputRecording(options: MeasurementLaunchOptions, engine: EngineManager) {
        guard let seconds = options.recordInputSeconds else { return }

        Task {
            while UIApplication.shared.applicationState != .active {
                try? await Task.sleep(for: .milliseconds(100))
            }

            if options.openFileName != nil, await !Self.waitForFullFrame(in: engine.frames) {
                print("input recording: no full frame")
            }

            UIApplication.shared.isIdleTimerDisabled = true
            await InputRecorder.shared.record(for: seconds)
            UIApplication.shared.isIdleTimerDisabled = false

            if options.exitsAfterSweep { exit(0) }
        }
    }
    #endif

    /// runMeasurement opens the photo named by `-UnveilOpen` and runs the sweep of `-UnveilSweep`
    /// (see MeasurementLaunchOptions), so T15's runs need no taps. The open goes through the
    /// session, like a pick on the home, so a good open presents the editor. Without
    /// `-UnveilOpen` it does nothing, and a normal launch is unchanged.
    ///
    /// Before the open it waits for an `app.gpu` read on the engine's serial command queue, which
    /// lands behind the launch read (the first one creates the GPU device): so `OpenToFirstFrame`
    /// starts after that cost, and the launch read shows apart as its own `Command` interval.
    ///
    /// With `-UnveilExitAfterSweep` the process ends when the sweep does, `exit(0)` only when it ran
    /// its full time, and `exit(1)` if the photo never showed a full frame. Like `-UnveilRun golden`,
    /// the exit is allowed only in this agent-driven mode, where nobody is editing.
    private func runMeasurement(
        options: MeasurementLaunchOptions,
        session: EditorSession<EngineManager, PhotoImporter, PhotoLibraryLoader>,
        canvas : CanvasView,
        engine : EngineManager
    ) {
        guard let fileName = options.openFileName else { return }

        let viewModel = session.viewModel

        Task {
            while UIApplication.shared.applicationState != .active {
                try? await Task.sleep(for: .milliseconds(100))
            }

            try? await Task.sleep(for: .seconds(options.delaySeconds))
            _ = try? await engine.diagnostic("app.gpu", params: [String: String]())

            // The bridge's timers and logs are off in a normal launch: only these options switch them on.
            if options.reportsTimeline || options.reportsMemoryMap || options.reportsCost || options.reportsDecodeMemory
                || options.poolLimitBytes != nil || options.disablesPoolIdle || options.probesPool {
                _ = try? await engine.diagnostic("unveil.diagnostics", params: ["enabled": true])
            }
            if let limit = options.poolLimitBytes {
                _ = try? await engine.diagnostic("unveil.pool", params: ["limitBytes": limit])
            }
            if options.disablesPoolIdle || options.probesPool {
                _ = try? await engine.diagnostic("unveil.pool", params: ["idleTrim": false])
            }

            let rawURL = URL.documentsDirectory.appending(path: "raw/\(fileName)")

            if options.reportsTimeline {
                await Self.prepareTimeline(for: rawURL, engine: engine)
            }

            if options.reportsTimeline || options.reportsMemoryMap {
                OpenTimeline.start()
            }

            let decodeSampler = options.reportsDecodeMemory ? DecodeMemorySampler() : nil
            if let decodeSampler {
                _ = try? await engine.diagnostic("unveil.decode-memory", params: ["enabled": true])
                decodeSampler.start(device: engine.frames.buffer(at: 0).device)
            }

            print("measure: opening \(fileName)")
            await session.open(pickedURL: rawURL)

            guard await Self.waitForFullFrame(in: engine.frames) else {
                await decodeSampler?.stop()
                if options.reportsDecodeMemory {
                    _ = try? await engine.diagnostic("unveil.decode-memory", params: ["enabled": false])
                }
                print("measure: no full frame for \(fileName): \(viewModel.errorMessage ?? "timed out")")
                if options.exitsAfterSweep { exit(1) }
                return
            }

            print("measure: first full frame")

            let firstFrameAt = ContinuousClock.now

            if options.reportsMemoryMap {
                await Self.printMemoryMap("open", since: firstFrameAt, engine: engine)
            }

            if options.reportsTimeline {
                try? await Task.sleep(for: .seconds(3))
                await Self.printTimeline(engine: engine)

                if options.exitsAfterSweep, !options.reportsMemoryMap, !options.reportsCost,
                   !options.reportsDecodeMemory, options.sweepSeconds == nil {
                    exit(0)
                }
            } else if options.reportsMemoryMap {
                _ = OpenTimeline.stop()
            }

            if options.reportsMemoryMap {
                let completed = await Self.runMemoryMap(
                    canvas,
                    engine       : engine,
                    firstFrameAt : firstFrameAt,
                    cycles       : options.memoryCycles
                )
                await decodeSampler?.stop()
                if options.reportsDecodeMemory {
                    let trace = (try? await engine.diagnostic("unveil.decode-memory", params: [String: String]())) ?? "{}"
                    print("DM-ALLOC " + trace)
                    _ = try? await engine.diagnostic("unveil.decode-memory", params: ["enabled": false])
                }
                if options.exitsAfterSweep { exit(completed ? 0 : 1) }
                return
            }

            await decodeSampler?.stop()
            if options.reportsDecodeMemory {
                let trace = (try? await engine.diagnostic("unveil.decode-memory", params: [String: String]())) ?? "{}"
                print("DM-ALLOC " + trace)
                _ = try? await engine.diagnostic("unveil.decode-memory", params: ["enabled": false])
            }

            if options.probesPool {
                let completed = await Self.probePool(
                    viewModel,
                    engine       : engine,
                    firstFrameAt : firstFrameAt
                )
                if options.exitsAfterSweep { exit(completed ? 0 : 1) }
                return
            }

            if options.reportsDecodeMemory, options.exitsAfterSweep, !options.reportsCost,
               options.sweepSeconds == nil, !options.cyclesZoom {
                exit(0)
            }

            if let panel = options.openPanel {
                session.showPanel(panel)
            }

            if let percent = options.zoomPercent {
                canvas.zoom(toPercent: percent)
            }

            if let exposure = options.exposure {
                await viewModel.drag(.exposure, to: exposure)
                await viewModel.endDrag(.exposure)
            }

            if options.showsClipping {
                session.clipping = [.shadows, .highlights]
            }

            if options.showsBefore {
                await viewModel.setShowingBefore(true)
            }

            if options.cyclesZoom {
                await Self.cycleZoom(canvas, engine: engine)
                if options.exitsAfterSweep { exit(0) }
                return
            }

            if options.reportsCost {
                await Self.settleBeforeCost(canvas, engine: engine, isZoomed: options.zoomPercent != nil)
            }

            guard let seconds = options.sweepSeconds else {
                if options.reportsCost {
                    await Self.printIdleCost(seconds: 30, engine: engine)
                    if options.exitsAfterSweep { exit(0) }
                }

                return
            }

            // A Debug build also writes the main thread's time during the sweep (FrameTimingProbe).
            let probe = FrameTimingProbe.shared
            let cost = options.reportsCost ? await Self.beginCost(engine: engine) : nil
            let sweep = StressSweep(
                viewModel         : viewModel,
                duration          : seconds,
                releasesEveryTick : options.releasesEveryTick
            ) { isComplete, csv in
                canvas.isMeasurementPanning = false

                Task {
                    if let cost {
                        await Self.printCost(cost, label: "sweep", isComplete: isComplete, engine: engine)
                    }

                    await probe.writeReport(to: csv.deletingPathExtension().appendingPathExtension("timing.csv"))
                    if options.exitsAfterSweep { exit(isComplete ? 0 : 1) }
                }
            }

            probe.start(phase: "sweep")
            sweep.start()

            if options.pansDuringSweep {
                // A pan stops a running spring: let the zoom animation land first.
                try? await Task.sleep(for: .seconds(1))
                await Self.pan(canvas, for: seconds - 1)
            }
        }
    }

    /// pan moves the canvas along its measurement circle about every 16 ms for `seconds`, the pan
    /// half of a `-UnveilPanSweep` run, then lets it rest so the window is rendered at full quality.
    private static func pan(_ canvas: CanvasView, for seconds: Double) async {
        let start = ContinuousClock.now

        canvas.isMeasurementPanning = true

        while canvas.isMeasurementPanning, start.duration(to: .now) < .seconds(seconds) {
            let elapsed = start.duration(to: .now) / .seconds(1)
            canvas.measurementPan(at: elapsed)

            try? await Task.sleep(for: .milliseconds(16))
        }

        canvas.isMeasurementPanning = false
    }

    /// cycleZoom is the `-UnveilZoomCycle` run, with FootprintSampler writing the memory CSV: 20 s at
    /// fit, then three times 100 % and back to fit. The first zoom is cold, the second follows 20 s
    /// at fit (the window memory was released 2 s in), the third only 1 s (nothing released). It
    /// prints when each phase starts, how long each zoom took from its start (spring included) to
    /// its first window and first full window, and the engine's `library.memory` after each hold.
    /// Then it pinches to 400 % and back, 1.5 s each way, with a rest after each. Every phase is a
    /// FrameTimingProbe phase; a Debug build writes their table next to the memory CSV.
    private static func cycleZoom(_ canvas: CanvasView, engine: EngineManager) async {
        let sampler = FootprintSampler()
        let start   = ContinuousClock.now
        let frames  = engine.frames

        func report(_ phase: String) async {
            let memory = (try? await engine.diagnostic("library.memory", params: [String: String]())) ?? "?"
            print("cycle: \(phase) memory \(memory)")
        }

        let probe = FrameTimingProbe.shared

        UIApplication.shared.isIdleTimerDisabled = true
        sampler.start()
        probe.start(phase: "fit-0")
        print("cycle: fit at \(start.duration(to: .now))")
        try? await Task.sleep(for: .seconds(20))
        await report("fit")

        for (round, zoomedHold, fitHold) in [(1, 20, 20), (2, 10, 1), (3, 10, 20)] {
            let before = frames.published().window?.generation ?? 0
            let zoomed = ContinuousClock.now
            var first  : Duration?

            print("cycle: zoom \(round) at \(start.duration(to: zoomed))")
            probe.mark("zoom-in-\(round)")
            canvas.zoom(toPercent: 100)

            while zoomed.duration(to: .now) < .seconds(30) {
                if let window = frames.published().window, window.generation > before {
                    first = first ?? zoomed.duration(to: .now)
                    if !window.isDraft { break }
                }

                try? await Task.sleep(for: .milliseconds(5))
            }

            let full = zoomed.duration(to: .now)
            print("cycle: zoom \(round) first window after \(first ?? full), first full window after \(full)")
            probe.mark("zoomed-\(round)")
            try? await Task.sleep(for: .seconds(zoomedHold))
            await report("zoom \(round)")

            print("cycle: fit \(round) at \(start.duration(to: .now))")
            probe.mark("zoom-out-\(round)")
            canvas.zoomToFit()

            // The spring and the loss of the window first, then the release 2 s after it.
            if fitHold > 4 {
                try? await Task.sleep(for: .seconds(1))
                probe.mark("release-\(round)")
                try? await Task.sleep(for: .seconds(3))
                probe.mark("fit-\(round)")
                try? await Task.sleep(for: .seconds(fitHold - 4))
            } else {
                try? await Task.sleep(for: .seconds(fitHold))
            }

            await report("fit \(round)")
        }

        // Fit to 400 % and back, each followed by a rest that lets the full window land.
        let pinches = [
            (phase: "pinch-in", from: 0.0, to: 1.0, rest: 4),
            (phase: "pinch-out", from: 1.0, to: 0.0, rest: 5),
        ]

        for step in pinches {
            await pinch(
                canvas,
                phase     : step.phase,
                toPercent : 400,
                from      : step.from,
                to        : step.to
            )
            probe.mark("\(step.phase)-rest")
            try? await Task.sleep(for: .seconds(step.rest))
        }

        await probe.writeReport(to: sampler.fileURL.deletingPathExtension().appendingPathExtension("timing.csv"))
        await sampler.stop()
        UIApplication.shared.isIdleTimerDisabled = false
        print("cycle: done at \(start.duration(to: .now))")
    }

    /// pinch zooms the canvas like a 1.5 s finger pinch at its center (CanvasView.measurementPinch),
    /// in steps about every 8 ms, from `from` to `to` of the way between fit and `percent`.
    private static func pinch(
        _ canvas : CanvasView,
        phase    : String,
        toPercent: Double,
        from     : Double,
        to       : Double
    ) async {
        let start = ContinuousClock.now

        FrameTimingProbe.shared.mark(phase)
        canvas.isMeasurementPanning = true

        while start.duration(to: .now) < .seconds(1.5) {
            let progress = start.duration(to: .now) / .seconds(1.5)
            canvas.measurementPinch(toPercent: toPercent, progress: from + (to - from) * progress)

            try? await Task.sleep(for: .milliseconds(8))
        }

        canvas.measurementPinch(toPercent: toPercent, progress: to)
        canvas.isMeasurementPanning = false
    }

    /// waitForFullFrame polls the sink every 20 ms, for at most 60 s, until it holds a full frame.
    /// The sink starts empty at launch, so the first full frame is the open's preview.
    private static func waitForFullFrame(in frames: FrameSink) async -> Bool {
        for _ in 0 ..< 3000 {
            if let frame = frames.latest(), !frame.isDraft { return true }

            try? await Task.sleep(for: .milliseconds(20))
        }

        return false
    }

    /// exitIfAgentDriven ends the process with `exit(1)` when a launch argument asked for an
    /// agent-driven run (`-UnveilRun`, `-UnveilOpen`) and the engine is not there to run it: the
    /// devicectl caller then sees the failure at once instead of waiting for its own timeout.
    static func exitIfAgentDriven(reason: String) {
        let isAgentDriven = UserDefaults.standard.string(forKey: "UnveilRun") != nil
            || MeasurementLaunchOptions(arguments: ProcessInfo.processInfo.arguments).openFileName != nil

        guard isAgentDriven else { return }

        print("engine unavailable: \(reason)")
        exit(1)
    }
}
