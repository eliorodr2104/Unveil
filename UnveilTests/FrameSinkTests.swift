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
        _ pixels  : [UInt8],
        to sink   : FrameSink,
        width     : Int,
        height    : Int,
        generation: UInt64,
        isDraft   : Bool = false
    ) {
        pixels.withUnsafeBytes { raw in
            guard let base = raw.baseAddress else { return }

            sink.receive(
                rgba      : base,
                width     : width,
                height    : height,
                stride    : width * 4,
                generation: generation,
                isDraft   : isDraft
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
        let buffer = frame.bufferIndex == 0 ? sink.buffers.0 : sink.buffers.1
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
}
