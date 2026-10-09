//
//  DiagnosticsMenu.swift
//  Unveil
//

#if DEBUG

import UIKit

/// DiagnosticsMenu is the debug-only menu of the editor: tools to check the pixel path by hand.
///
/// "Inject test frame" writes a gradient straight into the FrameSink, so the canvas can be checked
/// without the engine. It works only while no photo is open, because then nothing else feeds the
/// sink and its single-producer rule holds. The frame carries generation 0, which the sink accepts
/// only while it is empty: the engine numbers its renders from 1, so the first real preview still
/// replaces the gradient. "Export reference images" and "10-min soak test" stay empty until T14
/// and T15 fill them.
enum DiagnosticsMenu {

    private static let gradientSide = 512

    /// make builds the menu. `isPhotoOpen` is asked at tap time, not when the menu is built.
    static func make(
        frames     : FrameSink,
        isPhotoOpen: @escaping @MainActor () -> Bool
    ) -> UIMenu {
        let injectTestFrame = UIAction(
            title : "Inject test frame",
            image : UIImage(systemName: "square.fill")
        ) { _ in
            guard !isPhotoOpen() else { return }

            injectGradient(into: frames)
        }

        let exportReferenceImages = UIAction(
            title      : "Export reference images",
            attributes : .disabled
        ) { _ in }

        let soakTest = UIAction(
            title      : "10-min soak test",
            attributes : .disabled
        ) { _ in }

        return UIMenu(
            title    : "Diagnostics",
            image    : UIImage(systemName: "ladybug"),
            children : [injectTestFrame, exportReferenceImages, soakTest]
        )
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

#endif
