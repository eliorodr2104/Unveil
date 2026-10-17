//
//  FrameSinkTests.swift
//  UnveilTests
//

import Metal
import os
import Testing
@testable import Unveil

struct FrameSinkTests {

    let device: any MTLDevice

    init() throws {
        device = try #require(MTLCreateSystemDefaultDevice())
    }

    /// deliver hands `pixels` to the sink as a tightly packed frame, the way the engine does.
    private func deliver(
        _ pixels      : [UInt8],
        to sink       : FrameSink,
        width         : Int,
        height        : Int,
        generation    : UInt64,
        isDraft       : Bool = false,
        window        : FrameWindow? = nil,
        replacesWindow: Bool = false
    ) {
        pixels.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }

            sink.receive(
                rgba          : base,
                width         : width,
                height        : height,
                stride        : width * 4,
                generation    : generation,
                isDraft       : isDraft,
                window        : window,
                replacesWindow: replacesWindow,
                nativeWidth   : 640,
                nativeHeight  : 427
            )
        }
    }

    @Test
    func aFrameIsPublishedWithAlignedRows() {
        let sink = FrameSink(device: device, maxPixels: 64)

        deliver(
            [UInt8](repeating: 200, count: 10 * 3 * 4),
            to         : sink,
            width      : 10,
            height     : 3,
            generation : 1
        )

        let frame = sink.latest()
        #expect(frame?.generation == 1)
        #expect((frame?.bytesPerRow ?? 0) >= 40)
        #expect((frame?.bytesPerRow ?? 0) % device.minimumLinearTextureAlignment(for: .rgba8Unorm) == 0)
    }

    @Test
    func anOlderGenerationIsIgnored() {
        let sink   = FrameSink(device: device, maxPixels: 64)
        let pixels = [UInt8](repeating: 1, count: 4 * 4 * 4)

        deliver(
            pixels,
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 5
        )
        deliver(
            pixels,
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 3,
            isDraft    : true
        )

        #expect(sink.latest()?.generation == 5)
    }

    @Test
    func consecutiveFramesAlternateBuffers() {
        let sink   = FrameSink(device: device, maxPixels: 64)
        let pixels = [UInt8](repeating: 1, count: 4 * 4 * 4)

        deliver(
            pixels,
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 1
        )
        let first = sink.latest()?.bufferIndex

        deliver(
            pixels,
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 2
        )
        let second = sink.latest()?.bufferIndex

        #expect(first != nil)
        #expect(first != second)
    }

    @Test
    func anOversizedFrameIsDropped() {
        let sink   = FrameSink(device: device, maxPixels: 8)
        let pixels = [UInt8](repeating: 1, count: 16 * 16 * 4)

        deliver(
            pixels,
            to         : sink,
            width      : 16,
            height     : 16,
            generation : 1
        )

        #expect(sink.latest() == nil)
    }

    @Test
    func aDroppedFrameLeavesTheBufferIndexAlone() {
        let sink   = FrameSink(device: device, maxPixels: 8)
        let pixels = [UInt8](repeating: 1, count: 4 * 4 * 4)
        let big    = [UInt8](repeating: 1, count: 16 * 16 * 4)

        deliver(
            pixels,
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 1
        )
        deliver(
            big,
            to         : sink,
            width      : 16,
            height     : 16,
            generation : 2
        )
        deliver(
            pixels,
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 3
        )

        #expect(sink.latest()?.generation == 3)
        #expect(sink.latest()?.bufferIndex == 1)
    }

    @Test
    func theLimitIsTheLongEdgeNotThePixelCount() {
        let sink = FrameSink(device: device, maxPixels: 16)

        // 16x2 has 32 pixels, over 16, but its long edge fits.
        deliver(
            [UInt8](repeating: 1, count: 16 * 2 * 4),
            to         : sink,
            width      : 16,
            height     : 2,
            generation : 1
        )
        #expect(sink.latest()?.width == 16)

        // A tall frame past the edge is dropped even though it is thin.
        deliver(
            [UInt8](repeating: 1, count: 2 * 17 * 4),
            to         : sink,
            width      : 2,
            height     : 17,
            generation : 2
        )
        #expect(sink.latest()?.generation == 1)
    }

    @Test
    func rowsLandAtTheAlignedStrideWithTheirOwnPixels() throws {
        let sink = FrameSink(device: device, maxPixels: 64)

        // Three rows of five pixels: row r is filled with the byte r + 10.
        var pixels = [UInt8]()
        for row in 0 ..< 3 {
            pixels += [UInt8](repeating: UInt8(row + 10), count: 5 * 4)
        }
        deliver(
            pixels,
            to         : sink,
            width      : 5,
            height     : 3,
            generation : 1
        )

        let frame  = try #require(sink.latest())
        let buffer = sink.buffer(at: frame.bufferIndex)
        let bytes  = buffer.contents().assumingMemoryBound(to: UInt8.self)

        for row in 0 ..< 3 {
            for column in 0 ..< 5 * 4 {
                #expect(bytes[row * frame.bytesPerRow + column] == UInt8(row + 10))
            }
        }
    }

    @Test
    func onFrameRunsAfterPublishingAndMayReenterTheSink() {
        let sink = FrameSink(device: device, maxPixels: 64)
        let seen = OSAllocatedUnfairLock<UInt64?>(initialState: nil)

        // latest() takes the sink's lock: if onFrame ran inside it, Mutex traps on re-entry.
        sink.onFrame = { seen.withLock { $0 = sink.latest()?.generation } }

        deliver(
            [UInt8](repeating: 1, count: 4 * 4 * 4),
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 7
        )

        #expect(seen.withLock { $0 } == 7)
    }

    /// Windows and previews alternate and repeat, as during a pan after an edit: each kind keeps its
    /// own published buffer, a window never overwrites the preview's pixels, and the generation
    /// rule holds across the two kinds.
    @Test
    func aWindowIsPublishedBesideThePreviewWithoutTouchingItsPixels() throws {
        let sink   = FrameSink(device: device, maxPixels: 64)
        let window = FrameWindow(
            frameWidth  : 400,
            frameHeight : 300,
            x           : 100,
            y           : 50,
            width       : 4,
            height      : 4
        )

        deliver(
            [UInt8](repeating: 9, count: 4 * 4 * 4),
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 1
        )

        for generation in UInt64(2) ... 5 {
            deliver(
                [UInt8](repeating: UInt8(generation), count: 4 * 4 * 4),
                to         : sink,
                width      : 4,
                height     : 4,
                generation : generation,
                window     : window
            )
        }

        let published = sink.published()
        let preview   = try #require(published.frame)
        let shown     = try #require(published.window)
        let bytes     = sink.buffer(at: preview.bufferIndex).contents().assumingMemoryBound(to: UInt8.self)

        #expect(preview.generation == 1 && preview.window == nil)
        #expect(shown.generation == 5 && shown.window == window)
        #expect(preview.bufferIndex != shown.bufferIndex)
        #expect(bytes[0] == 9, "a window overwrote the preview")

        // An older window is dropped, and a newer preview takes the free buffer.
        deliver(
            [UInt8](repeating: 1, count: 4 * 4 * 4),
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 4,
            window     : window
        )
        deliver(
            [UInt8](repeating: 6, count: 4 * 4 * 4),
            to         : sink,
            width      : 4,
            height     : 4,
            generation : 6
        )

        let after = sink.published()
        #expect(after.window?.generation == 5)
        #expect(after.frame?.generation == 6)
        #expect(Set([after.frame?.bufferIndex, after.window?.bufferIndex]).count == 2)
    }

    /// The engine's whole-frame answer to a window it could not render alone unpublishes the window,
    /// which is older and may hold older settings; an ordinary preview leaves the window in place.
    /// Every frame keeps the native size it came with.
    @Test
    func aFallbackPreviewUnpublishesTheWindow() {
        let sink   = FrameSink(device: device, maxPixels: 64)
        let pixels = [UInt8](repeating: 7, count: 4 * 4 * 4)
        let window = FrameWindow(
            frameWidth  : 400,
            frameHeight : 300,
            x           : 100,
            y           : 50,
            width       : 4,
            height      : 4
        )

        deliver(pixels, to: sink, width: 4, height: 4, generation: 1)
        deliver(pixels, to: sink, width: 4, height: 4, generation: 2, window: window)
        deliver(pixels, to: sink, width: 4, height: 4, generation: 3)
        #expect(sink.published().window?.generation == 2)

        deliver(pixels, to: sink, width: 4, height: 4, generation: 4, replacesWindow: true)

        let published = sink.published()
        #expect(published.frame?.generation == 4)
        #expect(published.window == nil)
        #expect(published.frame?.nativeWidth == 640 && published.frame?.nativeHeight == 427)
    }

    @Test
    func aReducedRasterKeepsLogicalCoverageWithoutInflatingItsBuffer() throws {
        let sink = FrameSink(device: device, maxPixels: 128)
        let placement = FrameWindow(
            frameWidth : 2051,
            frameHeight: 1367,
            x          : 51,
            y          : 43,
            width      : 173,
            height     : 111
        )
        let pixels = [UInt8](repeating: 91, count: 87 * 56 * 4)
        pixels.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }

            sink.receive(
                rgba         : base,
                width        : 87,
                height       : 56,
                stride       : 87 * 4,
                generation   : 1,
                isDraft      : true,
                window       : placement,
                displayWidth : 173,
                displayHeight: 111
            )
        }
        let frame = try #require(sink.published().window)
        #expect(frame.width == 87 && frame.height == 56)
        #expect(frame.displayWidth == 173 && frame.displayHeight == 111)
        #expect(frame.window == placement)
        #expect(!frame.hasIdentityRaster)
        let bytes = sink.buffer(at: frame.bufferIndex).contents().assumingMemoryBound(to: UInt8.self)
        #expect(bytes[55 * frame.bytesPerRow + 86 * 4 + 3] == 91)
    }

    @Test
    func invalidLogicalExtentsCannotReplaceTheLastGoodFrame() {
        let sink   = FrameSink(device: device, maxPixels: 8)
        let pixels = [UInt8](repeating: 20, count: 4 * 3 * 4)
        deliver(
            pixels,
            to        : sink,
            width     : 4,
            height    : 3,
            generation: 1
        )
        pixels.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }

            sink.receive(
                rgba         : base,
                width        : 4,
                height       : 3,
                stride       : 16,
                generation   : 2,
                isDraft      : true,
                displayWidth : 0,
                displayHeight: 3
            )
        }
        #expect(sink.latest()?.generation == 1)
    }
}
