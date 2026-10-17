//
//  EditorLaunchRunner+Measurements.swift
//  Unveil
//

import UIKit

/// The E0 measurement runs of EditorLaunchRunner: the memory map, the open timeline, the cost
/// totals around a sweep and the Metal calibration. They are called only by the launch options
/// `-UnveilMemoryMap`, `-UnveilTimeline`, `-UnveilCost` and `-UnveilRun metal`.
extension EditorLaunchRunner {

    /// CostStart is the counters at the start of a measured stretch (`-UnveilCost`).
    struct CostStart {

        let sample: ProcessCostProbe.Sample
    }

    /// settleBeforeCost waits for the open's own work to end before a cost sweep starts: 3 s, and for
    /// a zoomed run the first full window, so the sweep measures steady renders, not the stage build.
    static func settleBeforeCost(
        _ canvas : CanvasView,
        engine   : EngineManager,
        isZoomed : Bool
    ) async {
        if isZoomed {
            let started = ContinuousClock.now

            while started.duration(to: .now) < .seconds(15) {
                if let window = engine.frames.published().window, !window.isDraft { break }

                try? await Task.sleep(for: .milliseconds(20))
            }
        }

        try? await Task.sleep(for: .seconds(3))
    }

    /// beginCost resets the engine's render totals and the canvas's GPU time and samples the process.
    static func beginCost(engine: EngineManager) async -> CostStart {
        _ = try? await engine.diagnostic("unveil.renders", params: ["reset": true])
        _ = try? await engine.diagnostic("unveil.pool", params: ["reset": true])
        CanvasGPUTime.reset()
        CanvasGPUTime.isActive = true
        SliderLatencyLog.start()

        return CostStart(sample: ProcessCostProbe.sample())
    }

    /// printCost prints the `COST` line: the process's deltas since `start`, the canvas's GPU passes
    /// and the engine's render totals per category (render thread, window histogram frame, delivery).
    static func printCost(
        _ start    : CostStart,
        label      : String,
        isComplete : Bool,
        engine     : EngineManager
    ) async {
        let end      = ProcessCostProbe.sample()
        let gpu      = CanvasGPUTime.totals()
        let renders  = (try? await engine.diagnostic("unveil.renders", params: ["reset": false])) ?? "{}"
        let pool     = (try? await engine.diagnostic("unveil.pool", params: ["reset": false])) ?? "{}"

        CanvasGPUTime.isActive = false

        let object: [String: Any] = [
            "label"     : label,
            "complete"  : isComplete,
            "thermal"   : FootprintSampler.name(of: ProcessInfo.processInfo.thermalState),
            "footprint" : FootprintSampler.physicalFootprint() ?? 0,
            "process"   : ProcessCostProbe.delta(from: start.sample, to: end),
            "canvasGpu" : ["passes": gpu.passes, "seconds": gpu.seconds, "longestSeconds": gpu.longest],
            "sliderToFrame": SliderLatencyLog.stop(),
            "renders"   : (try? JSONSerialization.jsonObject(with: Data(renders.utf8))) ?? renders,
            "pool"      : (try? JSONSerialization.jsonObject(with: Data(pool.utf8))) ?? pool,
        ]

        emit("COST", object)
    }

    /// printIdleCost measures `seconds` of an untouched editor: the power the app draws at rest.
    static func printIdleCost(seconds: Double, engine: EngineManager) async {
        let start = await beginCost(engine: engine)

        try? await Task.sleep(for: .seconds(seconds))
        await printCost(start, label: "idle", isComplete: true, engine: engine)
    }

    static func emit(_ prefix: String, _ object: [String: Any]) {
        let data = (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])) ?? Data()
        print(prefix + " " + String(decoding: data, as: UTF8.self))
    }

    /// calibrateMemory (`-UnveilRun metal`) shows what a shared Metal buffer and a malloc block cost the
    /// footprint, untouched, touched and freed, so the memory map can say how GPU bytes count.
    static func calibrateMemory(frames: FrameSink) async {
        let device = frames.buffer(at: 0).device
        let length = 256 << 20
        let start  = ContinuousClock.now

        func moment(_ name: String) {
            print(MemoryMapReport.line(moment: name, elapsed: start.duration(to: .now) / .seconds(1), engineJSON: "{}", frames: frames))
        }

        moment("base")

        let buffer = device.makeBuffer(length: length, options: .storageModeShared)
        moment("metal-allocated-untouched")

        if let buffer {
            memset(buffer.contents(), 1, length)
            moment("metal-touched")
        }

        moment("metal-still-held")
        try? await Task.sleep(for: .seconds(1))

        let block = UnsafeMutableRawPointer.allocate(byteCount: length, alignment: 16384)
        moment("malloc-allocated-untouched")
        memset(block, 1, length)
        moment("malloc-touched")
        block.deallocate()
        moment("malloc-freed")
        try? await Task.sleep(for: .seconds(1))
        moment("malloc-freed-1s")
        malloc_zone_pressure_relief(nil, 0)
        moment("malloc-after-pressure-relief")
        withExtendedLifetime(buffer) {}

        // Can a large block be given back? An mmap region unmapped, and a malloc block marked reusable.
        if let mapped = mmap(nil, length, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANON, -1, 0), mapped != MAP_FAILED {
            memset(mapped, 1, length)
            moment("mmap-touched")
            munmap(mapped, length)
            moment("munmap-freed")
        }

        let reusable = UnsafeMutableRawPointer.allocate(byteCount: length, alignment: 16384)
        memset(reusable, 1, length)
        moment("malloc2-touched")
        madvise(reusable, length, MADV_FREE_REUSABLE)
        moment("malloc2-madvise-free-reusable")
        reusable.deallocate()
        moment("malloc2-freed-after-madvise")
    }

    /// printLoads prints the engine's file loads since the last read as one `LOADS` line, so each
    /// stretch of a run (open, zoom, back to fit) shows how many decodes it caused.
    static func printLoads(_ label: String, engine: EngineManager) async {
        let json  = (try? await engine.diagnostic("unveil.loads", params: [String: String]())) ?? "{}"
        let loads = (try? JSONSerialization.jsonObject(with: Data(json.utf8))) ?? json

        emit("LOADS", ["label": label, "loads": loads])
    }

    /// printMemoryMap prints one `MM` moment: the engine's `library.memory` and the app's own counters.
    static func printMemoryMap(_ moment: String, since firstFrameAt: ContinuousClock.Instant, engine: EngineManager) async {
        let memory = (try? await engine.diagnostic("library.memory", params: [String: String]())) ?? "{}"

        print(MemoryMapReport.line(
            moment     : moment,
            elapsed    : firstFrameAt.duration(to: .now) / .seconds(1),
            engineJSON : memory,
            frames     : engine.frames
        ))
    }

    /// runMemoryMap is the rest of the `-UnveilMemoryMap` run, after the "open" moment: a memory line
    /// and a COST line (the power of the hold) at fit after 5 and 15 s, at 100 % once the first full window
    /// landed and 10 s more, and back at fit 10 s and 30 s after the zoom out (the window memory is
    /// released 2 s after it). FootprintSampler writes the footprint CSV meanwhile.
    static func runMemoryMap(
        _ canvas     : CanvasView,
        engine       : EngineManager,
        firstFrameAt : ContinuousClock.Instant,
        cycles       : Int = 1
    ) async -> Bool {
        let sampler = FootprintSampler()

        UIApplication.shared.isIdleTimerDisabled = true
        sampler.start()
        CanvasGPUTime.isActive = true

        func hold(_ seconds: Double, moment: String) async {
            let start = await beginCost(engine: engine)

            try? await Task.sleep(for: .seconds(seconds))
            await printCost(start, label: "hold-\(moment)", isComplete: true, engine: engine)
            await printMemoryMap(moment, since: firstFrameAt, engine: engine)
        }

        await printLoads("open-to-fit", engine: engine)
        await hold(max(0, 5 - firstFrameAt.duration(to: .now) / .seconds(1)), moment: "fit-5s")
        await hold(10, moment: "fit")
        await printLoads("fit-hold", engine: engine)

        for cycle in 1 ... cycles {
            func label(_ name: String) -> String { cycle == 1 ? name : "\(name)-cycle-\(cycle)" }
            let previousWindow = engine.frames.published().window?.generation ?? 0
            let zoomed = ContinuousClock.now
            canvas.zoom(toPercent: 100)

            while zoomed.duration(to: .now) < .seconds(30) {
                if let window = engine.frames.published().window,
                   window.generation > previousWindow,
                   !window.isDraft { break }

                try? await Task.sleep(for: .milliseconds(5))
            }

            guard let window = engine.frames.published().window,
                  window.generation > previousWindow,
                  !window.isDraft
            else {
                await sampler.stop()
                UIApplication.shared.isIdleTimerDisabled = false
                print("measure: full window timed out")
                return false
            }

            print("measure: first full window after \(zoomed.duration(to: .now))")
            await printLoads(label("zoom-in"), engine: engine)
            await hold(10, moment: label("zoom-100"))

            canvas.zoomToFit()
            await hold(10, moment: label("fit-after-10s"))
            await hold(20, moment: label("fit-after-30s"))
            await printLoads(label("zoom-out"), engine: engine)
        }

        await sampler.stop()
        UIApplication.shared.isIdleTimerDisabled = false
        print("measure: memory map done")
        return true
    }

    /// probePool isolates free-buffer reclamation after the photo has settled, then measures the
    /// first edit with the same live source and stages. This explicit probe disables automatic trim.
    static func probePool(
        _ viewModel  : EditorViewModel<EngineManager, PhotoImporter>,
        engine       : EngineManager,
        firstFrameAt : ContinuousClock.Instant
    ) async -> Bool {
        UIApplication.shared.isIdleTimerDisabled = true
        defer { UIApplication.shared.isIdleTimerDisabled = false }

        func edit(_ label: String, exposure: Double) async -> Bool {
            let previous = engine.frames.latest()?.generation ?? 0
            let cost = await beginCost(engine: engine)
            await viewModel.drag(.exposure, to: exposure)
            await viewModel.endDrag(.exposure)
            let started = ContinuousClock.now
            var completed = false
            while started.duration(to: .now) < .seconds(15) {
                if let frame = engine.frames.latest(), frame.generation > previous, !frame.isDraft {
                    completed = true
                    break
                }
                try? await Task.sleep(for: .milliseconds(5))
            }
            // Let the canvas present the accepted frame before reading presentation latency.
            try? await Task.sleep(for: .milliseconds(250))
            await printCost(cost, label: label, isComplete: completed, engine: engine)
            await printLoads(label, engine: engine)
            return completed
        }

        try? await Task.sleep(for: .seconds(15))
        guard await edit("pool-warm-edit", exposure: 0.25) else { return false }
        try? await Task.sleep(for: .seconds(15))
        await printMemoryMap("pool-before-trim", since: firstFrameAt, engine: engine)
        _ = try? await engine.diagnostic("unveil.pool", params: ["trimFreeBytes": UInt64(0)])
        try? await Task.sleep(for: .milliseconds(200))
        await printMemoryMap("pool-after-trim", since: firstFrameAt, engine: engine)
        return await edit("pool-first-after-trim", exposure: 0.5)
    }

    /// prepareTimeline clears the engine's load log (launch work is not the open) and times a cold copy
    /// of the file into an empty folder, the copy and hash an open of a new photo pays.
    static func prepareTimeline(for rawURL: URL, engine: EngineManager) async {
        _ = try? await engine.diagnostic("unveil.loads", params: [String: String]())

        let folder = URL.temporaryDirectory.appending(path: "e0-imports-\(UUID().uuidString)", directoryHint: .isDirectory)
        let start  = ProcessCostProbe.uptimeNs()

        _ = try? PhotoImporter(importsDirectory: folder).importCopy(of: rawURL)

        let milliseconds = Double(ProcessCostProbe.uptimeNs() - start) / 1_000_000
        try? FileManager.default.removeItem(at: folder)
        print("TL-COLD-COPY {\"ms\": \(milliseconds)}")
    }

    /// printTimeline merges the app's marks with the engine's file loads (decode, probe, embedded
    /// preview) into `TL` lines, in time order, in milliseconds from the open's first mark.
    static func printTimeline(engine: EngineManager) async {
        let marks = OpenTimeline.stop()
        let json  = (try? await engine.diagnostic("unveil.loads", params: [String: String]())) ?? "{}"
        let loads = ((try? JSONSerialization.jsonObject(with: Data(json.utf8))) as? [String: Any])?["loads"] as? [[String: Any]] ?? []
        let base  = marks.first?.nanoseconds ?? 0

        var rows = [(nanoseconds: UInt64, text: String)]()

        for mark in marks {
            rows.append((mark.nanoseconds, mark.name))
        }

        for load in loads {
            let start = (load["startNs"] as? NSNumber)?.uint64Value ?? 0
            let wall  = (load["wallNs"] as? NSNumber)?.uint64Value ?? 0
            let cpu   = (load["cpuNs"] as? NSNumber)?.uint64Value ?? 0
            let kind  = load["kind"] as? String ?? "?"
            let name  = load["name"] as? String ?? "?"
            let edge  = load["maxEdge"] as? Int ?? 0
            let size  = "\(load["width"] as? Int ?? 0)x\(load["height"] as? Int ?? 0)"

            rows.append((start, "load-begin \(kind) \(name) maxEdge \(edge)"))
            rows.append((start + wall, "load-end \(kind) \(name) maxEdge \(edge) size \(size) wallMs \(Double(wall) / 1e6) cpuMs \(Double(cpu) / 1e6)"))
        }

        for row in rows.sorted(by: { $0.nanoseconds < $1.nanoseconds }) {
            let milliseconds = (Double(row.nanoseconds) - Double(base)) / 1_000_000
            print("TL {\"tMs\": \(milliseconds), \"event\": \"\(row.text)\"}")
        }
    }
}
