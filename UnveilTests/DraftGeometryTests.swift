//
//  DraftGeometryTests.swift
//  UnveilTests
//

import CoreGraphics
import Testing
@testable import Unveil

/// DraftGeometryTests keeps raster sampling separate from framing, window requests and zoom anchors.
struct DraftGeometryTests {

    private func frame(
        raster : SIMD2<Int>,
        display: SIMD2<Int>,
        isDraft: Bool = true,
        window : FrameWindow? = nil
    ) -> ReadyFrame {
        ReadyFrame(
            bufferIndex  : 0,
            width        : raster.x,
            height       : raster.y,
            displayWidth : display.x,
            displayHeight: display.y,
            bytesPerRow  : raster.x * 4,
            generation   : 1,
            isDraft      : isDraft,
            window       : window,
            nativeWidth  : 6000,
            nativeHeight : 4000
        )
    }

    @Test(arguments: [(2051, 1367), (1367, 2051), (173, 111), (1, 1), (1, 37)])
    func reducedFramesKeepTheSameQuadAndVisibleWindow(size: (Int, Int)) {
        let display = SIMD2(size.0, size.1)
        let full = frame(
            raster : display,
            display: display,
            isDraft: false
        )
        let draft = frame(raster: SIMD2((size.0 + 1) / 2, (size.1 + 1) / 2), display: display)
        let transform = CanvasTransform(zoom: 3.5, offset: SIMD2(13.25, -27.75))
        let view      = CGSize(width: 900, height: 700)
        let fullQuad  = transform.quad(viewSize: view, imageSize: full.displaySize)
        let draftQuad = transform.quad(viewSize: view, imageSize: draft.displaySize)

        #expect(draft.displaySize == full.displaySize)
        #expect(draftQuad.min == fullQuad.min && draftQuad.max == fullQuad.max)

        let fullWindow = FrameWindow.visible(
            transform  : transform,
            viewSize   : view,
            imageSize  : full.displaySize,
            photoSize  : CGSize(width: 6000, height: 4000),
            screenScale: 2,
            maxPixels  : 2048
        )
        let draftWindow = FrameWindow.visible(
            transform  : transform,
            viewSize   : view,
            imageSize  : draft.displaySize,
            photoSize  : CGSize(width: 6000, height: 4000),
            screenScale: 2,
            maxPixels  : 2048
        )
        #expect(draftWindow == fullWindow)
    }

    @Test
    func anOddWindowKeepsItsLogicalEdgesAtHighZoom() throws {
        let placement = FrameWindow(
            frameWidth : 2051,
            frameHeight: 1367,
            x          : 51,
            y          : 43,
            width      : 173,
            height     : 111
        )
        let reduced = frame(
            raster : SIMD2(87, 56),
            display: SIMD2(173, 111),
            window : placement
        )
        let full = frame(
            raster : SIMD2(173, 111),
            display: SIMD2(173, 111),
            isDraft: false,
            window : placement
        )
        #expect(!reduced.hasIdentityRaster)
        #expect(full.hasIdentityRaster)
        #expect(reduced.displaySize == full.displaySize)

        let transform = CanvasTransform(zoom: 8, offset: SIMD2(29.5, -19.25))
        let image = transform.quad(
            viewSize : CGSize(width: 900, height: 700),
            imageSize: CGSize(width: 2051, height: 1367)
        )
        let reducedWindow = try #require(reduced.window)
        let fullWindow    = try #require(full.window)
        let reducedQuad   = reducedWindow.quad(in: image)
        let fullQuad      = fullWindow.quad(in: image)
        #expect(reducedQuad.min == fullQuad.min && reducedQuad.max == fullQuad.max)
    }

    @Test
    func draftsPreserveTheFullAnchorAndResetUsesTheNextPhotosFirstDraft() {
        var geometry = PreviewGeometry()
        let full = frame(
            raster : SIMD2(2051, 1367),
            display: SIMD2(2051, 1367),
            isDraft: false
        )
        let draft = frame(raster: SIMD2(1026, 684), display: SIMD2(2051, 1367))
        #expect(geometry.imageSize(for: draft) == CGSize(width: 2051, height: 1367))

        geometry.observe(full)
        for _ in 0 ..< 100 { geometry.observe(draft) }
        #expect(geometry.imageSize(for: draft) == full.displaySize)
        geometry.observe(full)
        #expect(geometry.imageSize(for: full) == full.displaySize)

        let cropped = frame(raster: SIMD2(684, 1026), display: SIMD2(1367, 2051))
        geometry.observe(cropped)
        #expect(geometry.imageSize(for: cropped) == full.displaySize)
        geometry.reset()
        #expect(geometry.imageSize(for: cropped) == CGSize(width: 1367, height: 2051))

        let nextFull = frame(
            raster : SIMD2(1367, 2051),
            display: SIMD2(1367, 2051),
            isDraft: false
        )
        geometry.observe(nextFull)
        #expect(geometry.imageSize(for: nextFull) == nextFull.displaySize)
    }

    @Test
    func aWindowCannotReplaceThePreviewAnchor() {
        var geometry = PreviewGeometry()
        let preview = frame(
            raster : SIMD2(2051, 1367),
            display: SIMD2(2051, 1367),
            isDraft: false
        )
        geometry.observe(preview)
        let window = FrameWindow(
            frameWidth : 6000,
            frameHeight: 4000,
            x          : 0,
            y          : 0,
            width      : 173,
            height     : 111
        )
        geometry.observe(frame(
            raster : SIMD2(173, 111),
            display: SIMD2(173, 111),
            isDraft: false,
            window : window
        ))
        #expect(geometry.fullSize == preview.displaySize)
    }
}
