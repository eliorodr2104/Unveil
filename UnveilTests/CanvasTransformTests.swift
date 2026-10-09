//
//  CanvasTransformTests.swift
//  UnveilTests
//

import CoreGraphics
import Testing
@testable import Unveil

struct CanvasTransformTests {

    @Test
    func aWideImageFitsTheWidth() {
        let quad = CanvasTransform().quad(
            viewSize  : CGSize(width: 1000, height: 1000),
            imageSize : CGSize(width: 2000, height: 1000)
        )

        #expect(quad.min.x == -1 && quad.max.x == 1)
        #expect(abs(quad.min.y + 0.5) < 1e-6 && abs(quad.max.y - 0.5) < 1e-6)
    }

    @Test
    func zoomIsClamped() {
        var transform = CanvasTransform()

        transform.pinch(by: 100, around: .zero, viewSize: CGSize(width: 100, height: 100))
        #expect(transform.zoom == 16)

        transform.pinch(by: 0.001, around: .zero, viewSize: CGSize(width: 100, height: 100))
        #expect(transform.zoom == 1)
    }

    @Test
    func pinchKeepsThePointUnderTheFingers() {
        var transform = CanvasTransform()
        let view      = CGSize(width: 1000, height: 1000)
        let image     = CGSize(width: 1000, height: 1000)
        let finger    = CGPoint(x: 750, y: 500)
        let before    = transform.imagePoint(at: finger, viewSize: view, imageSize: image)

        transform.pinch(by: 2, around: finger, viewSize: view)

        let after = transform.imagePoint(at: finger, viewSize: view, imageSize: image)
        #expect(abs(before.x - after.x) < 0.5 && abs(before.y - after.y) < 0.5)
    }

    @Test
    func resetReturnsToFit() {
        var transform = CanvasTransform()

        transform.pinch(by: 3, around: .zero, viewSize: CGSize(width: 100, height: 100))
        transform.pan(by: CGPoint(x: 40, y: 10))
        transform.reset()

        #expect(transform == CanvasTransform())
    }
}
