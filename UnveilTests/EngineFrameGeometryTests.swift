//
//  EngineFrameGeometryTests.swift
//  UnveilTests
//

import Metal
import Testing
import UnveilEngine
@testable import Unveil

/// EngineFrameGeometryTests exercises the C callback without creating an engine session.
struct EngineFrameGeometryTests {

    @Test
    func ABI7AppendsLogicalExtentAfterTheHistogram() {
        #expect(UV_ABI_VERSION == 7)
        #expect(MemoryLayout<UVFrame>.offset(of: \.display_width) == 80)
        #expect(MemoryLayout<UVFrame>.offset(of: \.display_height) == 84)
        #expect(MemoryLayout<UVFrame>.size == 88)
    }

    @Test
    func anOddDraftWindowKeepsLogicalPlacementAndPhysicalTexture() throws {
        let device = try #require(MTLCreateSystemDefaultDevice())
        let sink   = FrameSink(device: device, maxPixels: 128)
        deliver(
            to           : sink,
            width        : 87,
            height       : 56,
            displayWidth : 173,
            displayHeight: 111,
            window       : true
        )
        let frame = try #require(sink.published().window)
        let placement = FrameWindow(
            frameWidth : 2051,
            frameHeight: 1367,
            x          : 51,
            y          : 43,
            width      : 173,
            height     : 111
        )
        #expect(frame.width == 87 && frame.height == 56)
        #expect(frame.displayWidth == 173 && frame.displayHeight == 111)
        #expect(frame.window == placement)
        let texture = try #require(CanvasView.makeTexture(for: frame, in: sink))
        #expect(texture.width == 87 && texture.height == 56)
    }

    @Test
    func theFirstFitDraftCarriesTheFullLogicalExtent() throws {
        let device = try #require(MTLCreateSystemDefaultDevice())
        let sink   = FrameSink(device: device, maxPixels: 1100)
        deliver(
            to           : sink,
            width        : 1026,
            height       : 684,
            displayWidth : 2051,
            displayHeight: 1367,
            window       : false
        )
        let frame = try #require(sink.latest())
        #expect(frame.window == nil)
        #expect(frame.displayWidth == 2051 && frame.displayHeight == 1367)
        #expect(frame.nativeWidth == 6000 && frame.nativeHeight == 4000)
        #expect(!frame.hasIdentityRaster)
    }

    private func deliver(
        to sink      : FrameSink,
        width        : UInt32,
        height       : UInt32,
        displayWidth : UInt32,
        displayHeight: UInt32,
        window       : Bool
    ) {
        let pixels = [UInt8](repeating: 93, count: Int(width * height * 4))
        pixels.withUnsafeBufferPointer { bytes in
            var info = UVFrame()
            info.rgba           = bytes.baseAddress
            info.width          = width
            info.height         = height
            info.stride         = width * 4
            info.display_width  = displayWidth
            info.display_height = displayHeight
            info.frame_width    = 2051
            info.frame_height   = 1367
            info.x              = window ? 51 : 0
            info.y              = window ? 43 : 0
            info.native_width   = 6000
            info.native_height  = 4000
            info.generation     = 1
            info.draft          = true
            info.window         = window
            info.status         = UV_OK.rawValue
            withUnsafePointer(to: &info) { pointer in
                EngineManager.receiveFrame(Unmanaged.passUnretained(sink).toOpaque(), pointer)
            }
        }
    }
}
