//
//  GoldenExporter.swift
//  Unveil
//

import Foundation
import Metal
import os

// Nonisolated: it works off the main actor, and the app default is MainActor.
/// GoldenExporter renders every RAW in `Documents/raw` under five fixed adjustment presets and writes
/// each result as `Documents/golden/<raw-basename>__<preset>.png`: the iPad half of the equivalence
/// check against the Mac reference images, which are made by the same sequence in the Rust test.
///
/// The sequence per image goes only through the engine's public surface, as the editor does:
/// `openPhoto` (which imports and selects), `develop.reset` (the library keeps each photo's saved
/// edits, and presets must not stack on them), the preset's `set` calls, then one full
/// `requestPreview` at 2048 px. The wait polls the FrameSink every 20 ms off the main actor, and the
/// pixels are copied out of the shared buffer before the next request can overwrite them; the copy
/// honours the buffer's `bytesPerRow`. Nothing else may request a preview while it runs, or its frame
/// could be taken for ours.
///
/// A RAW that fails is reported by name and the run goes on, like the Mac test: that is a baseline
/// fact. `backend.json` (the `app.gpu` answer, read after the last render) is always written, and
/// `DONE` only when every image was written.
///
/// The export resets the edits of the photos it renders: it clears `Documents/golden` first (no stale
/// PNG survives a failed render), and it ends each RAW with `develop.reset`, so those photos are left
/// at their defaults and their previous saved edits are gone.
///
/// Known v0 limit: the editor's view model does not know the exporter changed the active photo, so
/// after a run its sliders no longer match the image. Relaunch the app to edit again.
nonisolated struct GoldenExporter<Engine: EngineDriving & EngineDiagnosing>: Sendable {

    /// Preset is one named set of develop values. The table below is the one in `raw-set.md`: the same
    /// names and numbers as the Mac test, so a file name says what was applied on both sides.
    struct Preset: Sendable {

        let name       : String
        let adjustments: [(kind: DevelopAdjustmentKind, value: Double)]
    }

    /// Summary is what a run did: how many PNGs it wrote and one line per failure.
    struct Summary: Sendable {

        let written : Int
        let failures: [String]
    }

    static var presets: [Preset] {
        [
            Preset(name: "neutral",     adjustments: []),
            Preset(name: "exposure",    adjustments: [(.exposure, 1.0)]),
            Preset(name: "contrast",    adjustments: [(.contrast, 60)]),
            Preset(name: "temperature", adjustments: [(.temperature, 4000)]),
            Preset(name: "tones",       adjustments: [(.shadows, 50), (.highlights, -50)]),
        ]
    }

    private static var rawExtensions: Set<String> { ["raf", "nef", "arw", "cr3", "dng"] }
    private static var previewPixels: Int          { 2048 }
    // One render takes 0.2 to 0.5 s on the iPad; a source the engine cannot load never answers.
    private static var renderTimeout: Duration     { .seconds(15) }

    private let engine         : Engine
    private let rawDirectory   : URL
    private let outputDirectory: URL
    private let logger         = Logger(subsystem: "com.eliorodr2104.unveil", category: "GoldenExporter")

    init(
        engine         : Engine,
        rawDirectory   : URL = URL.documentsDirectory.appending(path: "raw", directoryHint: .isDirectory),
        outputDirectory: URL = URL.documentsDirectory.appending(path: "golden", directoryHint: .isDirectory)
    ) {
        self.engine          = engine
        self.rawDirectory    = rawDirectory
        self.outputDirectory = outputDirectory
    }

    /// run exports every RAW under every preset and returns what it wrote and what failed. It runs
    /// off the main actor, and prints one progress line per image because `devicectl --console`
    /// shows stdout.
    @concurrent
    func run() async -> Summary {
        let fileManager = FileManager.default
        var written     = 0
        var failures    = [String]()

        do {
            try? fileManager.removeItem(at: outputDirectory)
            try fileManager.createDirectory(at: outputDirectory, withIntermediateDirectories: true)
        } catch {
            let reason = "cannot create \(outputDirectory.lastPathComponent): \(error.localizedDescription)"
            return Summary(written: 0, failures: [reason])
        }

        let raws = listRAWs()

        if raws.isEmpty {
            failures.append("no RAW files in \(rawDirectory.path(percentEncoded: false))")
        }

        for raw in raws {
            for preset in Self.presets {
                let name = "\(raw.deletingPathExtension().lastPathComponent)__\(preset.name)"

                do {
                    try await render(
                        raw,
                        preset : preset,
                        to     : outputDirectory.appending(path: "\(name).png")
                    )
                    written += 1
                    report("ok \(name)")
                } catch {
                    failures.append("\(name): \(error)")
                    report("FAILED \(name): \(error)")
                }
            }

            await resetToDefaults(afterRendering: raw)
        }

        await writeBackendAndDone(isComplete: failures.isEmpty)
        report("done: \(written) written, \(failures.count) failed")

        return Summary(written: written, failures: failures)
    }

    // MARK: - Steps

    private func listRAWs() -> [URL] {
        let files = (try? FileManager.default.contentsOfDirectory(
            at                         : rawDirectory,
            includingPropertiesForKeys : nil
        )) ?? []

        return files
            .filter { Self.rawExtensions.contains($0.pathExtension.lowercased()) }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    /// resetToDefaults leaves the active photo (the RAW just rendered) at its defaults, so the last
    /// preset is not kept as its saved edit.
    private func resetToDefaults(afterRendering raw: URL) async {
        do {
            _ = try await engine.diagnostic("develop.reset", params: [String: String]())
        } catch {
            report("cannot reset \(raw.lastPathComponent): \(error)")
        }
    }

    private func render(
        _ raw       : URL,
        preset      : Preset,
        to pngURL   : URL
    ) async throws {
        _ = try await engine.openPhoto(at: raw)
        _ = try await engine.diagnostic("develop.reset", params: [String: String]())

        for adjustment in preset.adjustments {
            try await engine.set(adjustment.kind, to: adjustment.value)
        }

        let generation = try engine.requestPreview(maxPixels: Self.previewPixels, draft: false)
        let (frame, pixels) = try await waitForFullFrame(atOrAbove: generation)

        try PNGWriter.write(
            rgba        : pixels,
            width       : frame.width,
            height      : frame.height,
            bytesPerRow : frame.bytesPerRow,
            to          : pngURL
        )
    }

    /// waitForFullFrame polls the sink until a non-draft frame at or above `generation` is published,
    /// then copies its rows out of the shared buffer, before anything can request the next render.
    ///
    /// The copy reads `bytesPerRow * height` bytes from the buffer the frame names. That is inside the
    /// buffer, which `FrameSink.init` sized for a square of the long edge with aligned rows.
    private func waitForFullFrame(atOrAbove generation: UInt64) async throws -> (ReadyFrame, Data) {
        let deadline = ContinuousClock.now + Self.renderTimeout

        while true {
            if let frame = engine.frames.latest(), frame.generation >= generation, !frame.isDraft {
                let buffer = frame.bufferIndex == 0 ? engine.frames.buffers.0 : engine.frames.buffers.1
                let length = frame.bytesPerRow * frame.height

                return (frame, Data(bytes: buffer.contents(), count: length))
            }

            guard ContinuousClock.now < deadline else {
                throw EngineError.timeout("no full frame for generation \(generation) within 120 s")
            }

            try await Task.sleep(for: .milliseconds(20))
        }
    }

    private func writeBackendAndDone(isComplete: Bool) async {
        do {
            let gpu = try await engine.diagnostic("app.gpu", params: [String: String]())
            try Data(gpu.utf8).write(to: outputDirectory.appending(path: "backend.json"))
        } catch {
            report("cannot record the backend: \(error)")
        }

        guard isComplete else { return }

        do {
            try Data().write(to: outputDirectory.appending(path: "DONE"))
        } catch {
            report("cannot write DONE: \(error)")
        }
    }

    private func report(_ line: String) {
        logger.info("golden \(line, privacy: .public)")
        print("golden \(line)")
    }
}
