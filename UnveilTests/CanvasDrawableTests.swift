//
//  CanvasDrawableTests.swift
//  UnveilTests
//

import Metal
import QuartzCore
import Testing
import UIKit
@testable import Unveil

/// CanvasDrawableTests holds real drawable callbacks to exercise the canvas handoff and lifecycle.
/// The controlled provider never waits on main; the tests decide when each acquisition completes.
@Suite(.serialized, .exclusiveWindow)
@MainActor
struct CanvasDrawableTests {

    @Test
    func aFinalDirtyFrameRetriesNilWithoutNewInput() async throws {
        let fixture = try Fixture()
        defer { fixture.close() }

        fixture.deliver(generation: 1, color: [0, 255, 0, 255])
        try await waitUntil(seconds: 3) { fixture.source.requestCount == 1 }
        try fixture.source.completeWithNil()

        try await waitUntil(seconds: 3) { fixture.source.requestCount == 2 }
        try await fixture.source.prepareDrawable()
        try fixture.source.completeWithDrawable()
        try await waitUntil(seconds: 3) { fixture.source.presentedCount == 1 }

        let pixel = try await fixture.centerPixel()
        #expect(pixel == [0, 255, 0, 255])
        try await Task.sleep(for: .milliseconds(250))
        #expect(fixture.source.requestCount == 2)
    }

    @Test
    func aHeldAcquisitionDrawsOnlyTheLatestCoalescedFrame() async throws {
        let fixture = try Fixture()
        defer { fixture.close() }

        fixture.deliver(generation: 1, color: [255, 0, 0, 255])
        try await waitUntil(seconds: 3) { fixture.source.requestCount == 1 }
        try await fixture.source.prepareDrawable()

        for generation in 2 ... 20 {
            fixture.deliver(generation: UInt64(generation), color: [0, 0, 255, 255])
        }
        fixture.deliver(generation: 21, color: [0, 255, 0, 255])
        try await Task.sleep(for: .milliseconds(80))
        #expect(fixture.source.requestCount == 1)

        try fixture.source.completeWithDrawable()
        try await waitUntil(seconds: 3) { fixture.source.presentedCount == 1 }
        let pixel = try await fixture.centerPixel()
        #expect(pixel == [0, 255, 0, 255])
    }

    @Test
    func resizeDiscardsTheHeldDrawableBeforeAcquiringTheNewSize() async throws {
        let fixture = try Fixture()
        defer { fixture.close() }

        fixture.deliver(generation: 1, color: [0, 255, 0, 255])
        try await waitUntil(seconds: 3) { fixture.source.requestCount == 1 }
        try await fixture.source.prepareDrawable()
        fixture.canvas.frame.size = CGSize(width: 48, height: 36)
        fixture.canvas.layoutIfNeeded()
        #expect(fixture.source.requestCount == 1)
        try fixture.source.completeWithDrawable()

        try await waitUntil(seconds: 3) { fixture.source.requestCount == 2 }
        #expect(fixture.source.presentedCount == 0)
        try await fixture.source.prepareDrawable()
        let prepared = try #require(fixture.source.prepared)
        #expect(prepared.drawable.texture.width == Int(48 * fixture.canvas.contentScaleFactor))
        try fixture.source.completeWithDrawable()
        try await waitUntil(seconds: 3) { fixture.source.presentedCount == 1 }
    }

    @Test
    func reattachmentRejectsThePreviousWindowsCallback() async throws {
        let fixture = try Fixture()
        defer { fixture.close() }

        fixture.deliver(generation: 1, color: [0, 255, 0, 255])
        try await waitUntil(seconds: 3) { fixture.source.requestCount == 1 }
        try await fixture.source.prepareDrawable()
        fixture.canvas.removeFromSuperview()
        fixture.root.view.addSubview(fixture.canvas)
        fixture.canvas.layoutIfNeeded()
        #expect(fixture.source.requestCount == 1)
        try fixture.source.completeWithDrawable()

        try await waitUntil(seconds: 3) { fixture.source.requestCount == 2 }
        #expect(fixture.source.presentedCount == 0)
        try await fixture.source.prepareDrawable()
        try fixture.source.completeWithDrawable()
        try await waitUntil(seconds: 3) { fixture.source.presentedCount == 1 }
    }

    @MainActor
    private final class Fixture {

        // any: Metal exposes devices and command queues only as protocol existentials.
        let device: any MTLDevice
        let sink  : FrameSink
        let canvas: CanvasView
        let source: ControlledDrawableSource
        let window: UIWindow
        let root  = UIViewController()

        init() throws {
            device = try #require(MTLCreateSystemDefaultDevice())
            let scene = try #require(
                UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first
            )
            sink = FrameSink(device: device, maxPixels: 64)
            var madeSource: ControlledDrawableSource?
            canvas = CanvasView(
                frames             : sink,
                device             : device,
                makeDrawableSource : { layer in
                    let source = ControlledDrawableSource(layer: layer)
                    madeSource = source
                    return source
                }
            )
            source = try #require(madeSource)
            window = UIWindow(windowScene: scene)
            canvas.frame = CGRect(x: 100, y: 100, width: 32, height: 24)
            let layer = try #require(canvas.layer as? CAMetalLayer)
            layer.framebufferOnly = false
            window.rootViewController = root
            root.view.addSubview(canvas)
            window.makeKeyAndVisible()
            window.layoutIfNeeded()
        }

        func close() {
            canvas.removeFromSuperview()
            source.cancelPending()
            window.isHidden = true
        }

        func deliver(generation: UInt64, color: [UInt8]) {
            let pixels = Array(repeating: color, count: 4 * 3).flatMap { $0 }
            pixels.withUnsafeBytes { raw in
                guard let base = raw.baseAddress else { return }

                sink.receive(
                    rgba         : base,
                    width        : 4,
                    height       : 3,
                    stride       : 16,
                    generation   : generation,
                    isDraft      : false,
                    nativeWidth  : 640,
                    nativeHeight : 480
                )
            }
        }

        /// centerPixel reads the texture retained before presentation, after its GPU work completed.
        /// The provider also retains the drawable so the layer cannot reuse it during readback.
        /// The test awaits the blit completion without blocking main.
        func centerPixel() async throws -> [UInt8] {
            let texture = try #require(source.deliveredTexture)
            let queue = try #require(device.makeCommandQueue())
            let command = try #require(queue.makeCommandBuffer())
            let buffer = try #require(device.makeBuffer(length: 256, options: .storageModeShared))
            let blit = try #require(command.makeBlitCommandEncoder())
            blit.copy(
                from                    : texture,
                sourceSlice             : 0,
                sourceLevel             : 0,
                sourceOrigin            : MTLOrigin(x: texture.width / 2, y: texture.height / 2, z: 0),
                sourceSize              : MTLSize(width: 1, height: 1, depth: 1),
                to                      : buffer,
                destinationOffset       : 0,
                destinationBytesPerRow  : 256,
                destinationBytesPerImage: 256
            )
            blit.endEncoding()
            let completed = await withCheckedContinuation { continuation in
                command.addCompletedHandler { command in
                    continuation.resume(returning: command.status == .completed)
                }
                command.commit()
            }
            #expect(completed)
            let bytes = buffer.contents().assumingMemoryBound(to: UInt8.self)
            return Array(UnsafeBufferPointer(start: bytes, count: 4))
        }
    }
}
