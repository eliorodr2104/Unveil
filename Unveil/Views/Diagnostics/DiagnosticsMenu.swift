//
//  DiagnosticsMenu.swift
//  Unveil
//

import UIKit

/// DiagnosticsMenu is the editor's diagnostics menu, present in every configuration: the baseline
/// measurements must run on a Release build, so the tools cannot be Debug-only in v0 (they get gated
/// before any App Store build).
///
/// "Inject test frame" writes a gradient straight into the FrameSink, so the canvas can be checked
/// without the engine. It works only while no photo is open, because then nothing else feeds the
/// sink and its single-producer rule holds. The frame carries generation 0, which the sink accepts
/// only while it is empty: the engine numbers its renders from 1, so the first real preview still
/// replaces the gradient.
///
/// "Engine state" shows `app.gpu`, `library.memory` and the canvas's possible tear count in an
/// alert, and appends the same reading to `engine.jsonl`. "Export reference images" runs
/// GoldenExporter and reports how it went. "10-min soak test" runs StressSweep for 600 s on the open
/// photo, through the editor, which owns the sweep and reports where the memory CSV went.
enum DiagnosticsMenu {

    private static let gradientSide = 512

    /// make builds the menu. `isPhotoOpen` and `possibleTearCount` are asked at tap time, not when the
    /// menu is built. `present` shows an alert on the editor; `startSoakTest` starts the sweep.
    static func make(
        engine           : some EngineDriving & EngineDiagnosing,
        isPhotoOpen      : @escaping @MainActor () -> Bool,
        possibleTearCount: @escaping @MainActor () -> Int,
        present          : @escaping @MainActor (UIAlertController) -> Void,
        startSoakTest    : @escaping @MainActor () -> Void
    ) -> UIMenu {
        let frames = engine.frames

        let injectTestFrame = UIAction(
            title : "Inject test frame",
            image : UIImage(systemName: "square.fill")
        ) { _ in
            guard !isPhotoOpen() else { return }

            injectGradient(into: frames)
        }

        let engineState = UIAction(
            title : "Engine state",
            image : UIImage(systemName: "cpu")
        ) { _ in
            Task {
                let alert = await engineStateAlert(engine: engine, possibleTears: possibleTearCount())
                present(alert)
            }
        }

        let exportReferenceImages = UIAction(
            title : "Export reference images",
            image : UIImage(systemName: "square.and.arrow.down")
        ) { _ in
            Task {
                let summary = await GoldenExporter(engine: engine).run()
                let message = describe(written: summary.written, failures: summary.failures)
                present(makeAlert(title: "Reference images", message: message))
            }
        }

        let soakTest = UIAction(
            title : "10-min soak test",
            image : UIImage(systemName: "flame")
        ) { _ in
            startSoakTest()
        }

        return UIMenu(
            title    : "Diagnostics",
            image    : UIImage(systemName: "ladybug"),
            children : [engineState, injectTestFrame, exportReferenceImages, soakTest]
        )
    }

    /// engineStateAlert reads the engine's GPU backend and memory, logs the reading and returns the
    /// alert that shows it. A failed read is shown too: it is a datum, not a reason to stay silent.
    private static func engineStateAlert(
        engine       : some EngineDiagnosing,
        possibleTears: Int
    ) async -> UIAlertController {
        let gpu: String
        let memory: String

        do {
            gpu    = try await engine.diagnostic("app.gpu", params: [String: String]())
            memory = try await engine.diagnostic("library.memory", params: [String: String]())
        } catch {
            return makeAlert(title: "Engine state", message: "Could not read it: \(error.message)")
        }

        EngineStateLog.append(
            event         : "menu",
            gpu           : gpu,
            possibleTears : possibleTears
        )

        let text = "app.gpu: \(gpu)\n\nlibrary.memory: \(memory)\n\npossibleTears: \(possibleTears)"
        return makeAlert(title: "Engine state", message: text)
    }

    private static func describe(written: Int, failures: [String]) -> String {
        let text = "\(written) images written to Documents/golden."
        guard !failures.isEmpty else { return text }

        return text + "\n\nFailed:\n" + failures.joined(separator: "\n")
    }

    private static func makeAlert(title: String, message: String) -> UIAlertController {
        let alert = UIAlertController(
            title          : title,
            message        : message,
            preferredStyle : .alert
        )

        alert.addAction(UIAlertAction(title: "OK", style: .default))
        return alert
    }

    /// injectGradient fills a square with red growing left to right and green growing top to bottom,
    /// so a flipped axis or swapped channels show at a glance, and hands it to the sink.
    private static func injectGradient(into frames: FrameSink) {
        let side     = gradientSide
        let rowBytes = side * 4
        var pixels   = [UInt8](repeating: 255, count: rowBytes * side)

        for row in 0 ..< side {
            for column in 0 ..< side {
                let offset = row * rowBytes + column * 4
                pixels[offset]     = UInt8(column * 255 / (side - 1))
                pixels[offset + 1] = UInt8(row * 255 / (side - 1))
                pixels[offset + 2] = 64
            }
        }

        pixels.withUnsafeBytes { bytes in
            guard let base = bytes.baseAddress else { return }

            frames.receive(
                rgba       : base,
                width      : side,
                height     : side,
                stride     : rowBytes,
                generation : 0,
                isDraft    : false
            )
        }
    }
}
