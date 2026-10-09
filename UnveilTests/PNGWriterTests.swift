//
//  PNGWriterTests.swift
//  UnveilTests
//

import CoreGraphics
import Foundation
import ImageIO
import Testing
@testable import Unveil

struct PNGWriterTests {

    /// A 4x3 buffer whose rows are padded to 20 bytes (16 of pixels, 4 of 0xEE), like a FrameSink row,
    /// and whose fourth byte of each pixel is 7: neither the padding nor that byte may reach the file.
    @Test
    func aKnownBufferDecodesToTheSameRGBBytes() throws {
        let width       = 4
        let height      = 3
        let bytesPerRow = 20
        var buffer      = Data(repeating: 0xEE, count: bytesPerRow * height)

        for row in 0 ..< height {
            for column in 0 ..< width {
                let offset = row * bytesPerRow + column * 4
                buffer[offset]     = UInt8(row * 50 + column)
                buffer[offset + 1] = UInt8(column * 60)
                buffer[offset + 2] = UInt8(200 - row * 10)
                buffer[offset + 3] = 7
            }
        }

        let url = FileManager.default.temporaryDirectory.appending(path: "png-writer-\(UUID()).png")
        defer { try? FileManager.default.removeItem(at: url) }

        try PNGWriter.write(
            rgba        : buffer,
            width       : width,
            height      : height,
            bytesPerRow : bytesPerRow,
            to          : url
        )

        let source = try #require(CGImageSourceCreateWithURL(url as CFURL, nil))
        let image  = try #require(CGImageSourceCreateImageAtIndex(source, 0, nil))
        #expect(image.width == width && image.height == height)

        // Redraw into a tight sRGB RGBX context: sRGB to sRGB, so the bytes come back unchanged.
        let space   = try #require(CGColorSpace(name: CGColorSpace.sRGB))
        let context = try #require(
            CGContext(
                data             : nil,
                width            : width,
                height           : height,
                bitsPerComponent : 8,
                bytesPerRow      : width * 4,
                space            : space,
                bitmapInfo       : CGImageAlphaInfo.noneSkipLast.rawValue
            )
        )
        context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))

        let decoded = try #require(context.data).assumingMemoryBound(to: UInt8.self)

        for row in 0 ..< height {
            for column in 0 ..< width {
                for channel in 0 ..< 3 {
                    let actual   = decoded[row * width * 4 + column * 4 + channel]
                    let expected = buffer[row * bytesPerRow + column * 4 + channel]

                    #expect(actual == expected)
                }
            }
        }
    }

    @Test
    func aBufferShorterThanItsGeometryIsRefused() {
        let url = FileManager.default.temporaryDirectory.appending(path: "png-writer-\(UUID()).png")

        #expect(throws: PNGWriteError.self) {
            try PNGWriter.write(
                rgba        : Data(count: 8),
                width       : 4,
                height      : 3,
                bytesPerRow : 16,
                to          : url
            )
        }
    }
}
