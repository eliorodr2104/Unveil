//
//  PNGWriter.swift
//  Unveil
//

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

// Nonisolated: the golden exporter writes off the main actor, and the app default is MainActor.
/// PNGWriter encodes an RGBA8 pixel buffer as an 8-bit RGB PNG: the alpha byte is dropped, never
/// written. The engine's bytes are already display-encoded sRGB, so the file gets the sRGB tag and
/// no conversion happens: the pixels in the PNG are the pixels the engine produced. The Mac
/// reference PNGs are the same thing (RGB8, no alpha), which is what makes the two sets comparable.
///
/// `bytesPerRow` is the buffer's own row stride, padding included: a FrameSink row is aligned for
/// Metal, so it is longer than `width * 4`, and reading it as tight would shear the image.
nonisolated enum PNGWriter {

    /// write encodes `rgba`, `height` rows of `bytesPerRow` bytes, each starting with `width` pixels
    /// of R, G, B and one ignored byte, and writes the file at `url`.
    static func write(
        rgba        : Data,
        width       : Int,
        height      : Int,
        bytesPerRow : Int,
        to url      : URL
    ) throws(PNGWriteError) {
        guard width > 0, height > 0, bytesPerRow >= width * 4, rgba.count >= bytesPerRow * height else {
            throw .badGeometry("\(width)x\(height), \(bytesPerRow) bytes per row, \(rgba.count) bytes")
        }

        // noneSkipLast: the fourth byte is padding, so ImageIO emits a colour-type-2 (RGB) PNG.
        let bitmapInfo = CGBitmapInfo(rawValue: CGImageAlphaInfo.noneSkipLast.rawValue)

        guard let space       = CGColorSpace(name: CGColorSpace.sRGB),
              let provider    = CGDataProvider(data: rgba as CFData),
              let image       = CGImage(
                  width             : width,
                  height            : height,
                  bitsPerComponent  : 8,
                  bitsPerPixel      : 32,
                  bytesPerRow       : bytesPerRow,
                  space             : space,
                  bitmapInfo        : bitmapInfo,
                  provider          : provider,
                  decode            : nil,
                  shouldInterpolate : false,
                  intent            : .defaultIntent
              ),
              let destination = CGImageDestinationCreateWithURL(
                  url as CFURL,
                  UTType.png.identifier as CFString,
                  1,
                  nil
              )
        else {
            throw .cannotEncode("cannot set up the encoder for \(url.lastPathComponent)")
        }

        CGImageDestinationAddImage(destination, image, nil)

        guard CGImageDestinationFinalize(destination) else {
            throw .cannotEncode("cannot write \(url.lastPathComponent)")
        }
    }
}
