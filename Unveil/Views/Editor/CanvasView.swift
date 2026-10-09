//
//  CanvasView.swift
//  Unveil
//

import Metal
import os
import QuartzCore
import Synchronization
import UIKit

/// CanvasView draws the newest ready frame of a FrameSink through its CAMetalLayer, with pinch to
/// zoom, pan, and a double tap that returns to aspect fit.
///
/// There is no polling. A CADisplayLink runs only while something is dirty: a new frame, a
/// transform change or a new drawable size marks the view, the next tick draws once, and the link
/// pauses again. A view with nothing to draw costs no CPU and no GPU.
///
/// New frames arrive on the engine thread through `frames.onFrame`. That callback never waits on
/// the main thread (a suspend in progress can hold main for up to 2 s, and the engine thread must
/// not stall behind it): it schedules at most one asynchronous hop, coalesced by an atomic flag,
/// and the hop only marks the view dirty. The pixels never cross threads: the tick reads
/// `frames.latest()` once, at encode time, and samples the buffer that snapshot names.
///
/// The textures are zero-copy views of the sink's shared buffers (`makeTexture(descriptor:offset:
/// bytesPerRow:)`), one cached per buffer and rebuilt only when the frame's geometry changes, so
/// steady drawing allocates no pixel memory. They are `rgba8Unorm`, the drawable is `bgra8Unorm`
/// and the layer's colorspace is sRGB: the engine's bytes are already display-encoded and travel
/// to the screen untouched (see CanvasShaderSource for the shader side).
///
/// The sink double-buffers, so a GPU pass that samples buffer A while the engine publishes twice
/// would read A as it is rewritten (a torn frame, visual only, gone on the next frame). Two rules
/// keep that rare and visible: at most one command buffer samples the sink at a time (a tick that
/// finds one in flight retries on the next tick), and the completed handler counts the passes
/// that may have torn, in `possibleTearCount`, so T13 can decide whether a third buffer is needed.
final class CanvasView: UIView {

    private static let drawablePixelFormat = MTLPixelFormat.bgra8Unorm
    private static let logger              = Logger(subsystem: "com.eliorodr2104.unveil", category: "CanvasView")

    // any: Metal vends its objects only as protocol existentials, there is no concrete type to name.
    private let frames      : FrameSink
    private let commandQueue: any MTLCommandQueue
    private let pipeline    : (any MTLRenderPipelineState)?
    private let renderPass  = MTLRenderPassDescriptor()
    private let crossThread = CrossThreadState()

    private var displayLink   : CADisplayLink?
    private var isDirty       : Bool = false
    private var laidOutSize   : CGSize = .zero
    private var cachedTextures: InlineArray<2, CachedTexture?> = [nil, nil]

    /// canvasTransform is the gesture state (UIView already owns `transform`). Any real change to
    /// it asks for one redraw.
    private var canvasTransform = CanvasTransform() {
        didSet {
            if canvasTransform != oldValue {
                setNeedsRedraw()
            }
        }
    }

    /// CachedTexture is the zero-copy texture over one sink buffer, valid while the frame in that
    /// buffer keeps the geometry it was made for. The buffer index is the slot it is stored in.
    private struct CachedTexture {

        let width      : Int
        let height     : Int
        let bytesPerRow: Int
        let texture    : any MTLTexture

        func fits(_ frame: ReadyFrame) -> Bool {
            width == frame.width && height == frame.height && bytesPerRow == frame.bytesPerRow
        }
    }

    /// CrossThreadState is what the engine thread and Metal's completion thread touch: only atomics,
    /// so it is Sendable without a lock. It is a separate object so those closures hold it, never the
    /// view: a UIView must not be retained, and possibly freed, off the main thread.
    // Nonisolated: the engine thread and Metal's completion thread use it, the app default is MainActor.
    private nonisolated final class CrossThreadState: Sendable {

        let isHopPending            = Atomic<Bool>(false)
        let isCommandBufferInFlight = Atomic<Bool>(false)
        let possibleTears           = Atomic<Int>(0)

        /// commandBufferCompleted(sampled:newest:) runs on Metal's completion thread when a pass ends.
        ///
        /// Any newer generation published while the pass ran is a possible tear. One publish lands
        /// in the other buffer and makes the sampled buffer the engine's next write target, so the
        /// engine may already be copying into it while the GPU still reads it; two or more publishes
        /// certainly rewrote it. The count is an upper bound: an exposed pass may still read clean.
        func commandBufferCompleted(sampled: ReadyFrame?, newest: ReadyFrame?) {
            if let sampled,
               let newest,
               newest.generation > sampled.generation {
                possibleTears.add(1, ordering: .relaxed)
            }

            isCommandBufferInFlight.store(false, ordering: .releasing)
        }
    }

    override class var layerClass: AnyClass { CAMetalLayer.self }

    /// possibleTearCount is the number of passes that ended with a newer frame published, so the
    /// engine may have rewritten the buffer they sampled (see CrossThreadState). A diagnostic for
    /// T13, not a correctness signal.
    var possibleTearCount: Int {
        crossThread.possibleTears.load(ordering: .relaxed)
    }

    // layerClass makes the backing layer a CAMetalLayer, so the downcast cannot fail.
    private var metalLayer: CAMetalLayer {
        unsafeDowncast(layer, to: CAMetalLayer.self)
    }

    /// init(frames:device:) builds the pipeline and wires the sink's wake-up to this view.
    ///
    /// The shaders compile here, from source, once per view. If they do not build, the error is
    /// logged and the canvas only clears to black: the editor stays usable, and CanvasTextureTests
    /// fails on the same error. A device that cannot make a command queue is out of resources at
    /// launch, with nothing to degrade to.
    init(frames: FrameSink, device: some MTLDevice) {
        guard let commandQueue = device.makeCommandQueue() else {
            fatalError("CanvasView: the device cannot make a command queue")
        }

        do {
            pipeline = try CanvasShaderSource.makePipelineState(
                device      : device,
                pixelFormat : Self.drawablePixelFormat
            )
        } catch {
            Self.logger.error("The canvas shaders did not build: \(String(describing: error), privacy: .public)")
            pipeline = nil
        }

        self.frames       = frames
        self.commandQueue = commandQueue

        super.init(frame: .zero)

        configureLayer(device: device)
        configureRenderPass()
        configureGestures()
        configureAccessibility()
        listenForFrames()
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("CanvasView is built in code, not from a storyboard")
    }

    /// setNeedsRedraw marks the canvas dirty: the next display link tick draws once, then the link
    /// pauses. Marks made before the next tick coalesce into that one draw.
    func setNeedsRedraw() {
        isDirty = true
        displayLink?.isPaused = false
    }

    // MARK: - View lifecycle

    /// didMoveToWindow owns the display link. CADisplayLink retains its target, so a link left
    /// alive after the view leaves its window would keep the view alive forever: it is invalidated
    /// on every move and made again only when there is a window to draw into.
    override func didMoveToWindow() {
        super.didMoveToWindow()

        displayLink?.invalidate()
        displayLink = nil

        guard let window else { return }

        contentScaleFactor = window.traitCollection.displayScale
        setNeedsLayout()

        let link = CADisplayLink(target: self, selector: #selector(displayLinkDidFire(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
        link.isPaused                = !isDirty
        link.add(to: .main, forMode: .common)

        displayLink = link
    }

    /// layoutSubviews sizes the drawable to the view in pixels; a new size needs one redraw.
    ///
    /// The comparison is against the last size laid out here, not `metalLayer.drawableSize`, so
    /// the redraw does not depend on whether CAMetalLayer ever resizes its drawable by itself.
    override func layoutSubviews() {
        super.layoutSubviews()

        let pixelSize = CGSize(
            width  : bounds.width  * contentScaleFactor,
            height : bounds.height * contentScaleFactor
        )

        guard pixelSize != laidOutSize else { return }

        laidOutSize             = pixelSize
        metalLayer.drawableSize = pixelSize
        setNeedsRedraw()
    }

    // MARK: - Drawing

    /// displayLinkDidFire draws once if the view is dirty, then pauses the link.
    ///
    /// If the last command buffer is still on the GPU the tick returns with the link running, so
    /// the draw is retried on the next tick: that is the one-pass-in-flight rule. `isDirty` is
    /// cleared before `encodeFrame` reads the sink, so a frame published after that read marks the
    /// view again and gets its own draw instead of being lost.
    @objc
    private func displayLinkDidFire(_ link: CADisplayLink) {
        guard isDirty else {
            link.isPaused = true
            return
        }

        let claimed = crossThread.isCommandBufferInFlight.compareExchange(
            expected : false,
            desired  : true,
            ordering : .acquiring
        )

        guard claimed.exchanged else { return }

        isDirty = false

        if !encodeFrame() {
            crossThread.isCommandBufferInFlight.store(false, ordering: .releasing)
        }

        link.isPaused = !isDirty
    }

    /// encodeFrame clears the drawable and draws the sink's newest frame into it, returning false
    /// when nothing was committed (no drawable yet, or no command buffer).
    ///
    /// `frames.latest()` is read exactly once, here, and both the texture and the tear check come
    /// from that one snapshot, so the pass never mixes the geometry of one frame with another.
    ///
    /// A nil drawable drops the draw instead of marking the view dirty again. Before the first
    /// layout the size is zero, and `layoutSubviews` marks the view once it has a size. Otherwise
    /// `nextDrawable` has already blocked main for its 1 s timeout, and retrying on every tick
    /// would chain more of those stalls: the next frame or gesture draws instead.
    private func encodeFrame() -> Bool {
        guard let commandBuffer = commandQueue.makeCommandBuffer(),
              let drawable      = metalLayer.nextDrawable()
        else {
            return false
        }

        let frame = frames.latest()

        // The encoder copies the descriptor, so the drawable is not kept past this pass.
        renderPass.colorAttachments[0].texture = drawable.texture
        let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: renderPass)
        renderPass.colorAttachments[0].texture = nil

        guard let encoder else { return false }

        var sampledFrame: ReadyFrame? = nil

        if let frame, let pipeline, let texture = texture(for: frame) {
            let quad = canvasTransform.quad(
                viewSize  : bounds.size,
                imageSize : CGSize(width: frame.width, height: frame.height)
            )
            var corners = SIMD4<Float>(lowHalf: quad.min, highHalf: quad.max)

            encoder.setRenderPipelineState(pipeline)
            encoder.setVertexBytes(&corners, length: MemoryLayout<SIMD4<Float>>.stride, index: 0)
            encoder.setFragmentTexture(texture, index: 0)
            encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)

            sampledFrame = frame
        }

        encoder.endEncoding()

        commandBuffer.present(drawable)
        commandBuffer.addCompletedHandler { [crossThread, frames, sampledFrame] _ in
            crossThread.commandBufferCompleted(sampled: sampledFrame, newest: frames.latest())
        }
        commandBuffer.commit()

        return true
    }

    /// texture(for:) returns the cached texture over the buffer holding `frame`, making a new one
    /// only when that buffer's frame changed size or stride.
    private func texture(for frame: ReadyFrame) -> (any MTLTexture)? {
        if let cached = cachedTextures[frame.bufferIndex], cached.fits(frame) {
            return cached.texture
        }

        guard let texture = Self.makeTexture(for: frame, in: frames) else {
            Self.logger.error("Cannot wrap buffer \(frame.bufferIndex) as a \(frame.width)x\(frame.height) texture")
            return nil
        }

        cachedTextures[frame.bufferIndex] = CachedTexture(
            width       : frame.width,
            height      : frame.height,
            bytesPerRow : frame.bytesPerRow,
            texture     : texture
        )

        return texture
    }

    /// makeTexture(for:in:) wraps the sink buffer that holds `frame` in an `rgba8Unorm` texture that
    /// shares its memory: no copy, and the texture sees every later write to that buffer.
    ///
    /// The bounds are the sink's contract: `bytesPerRow` is aligned to the device's linear texture
    /// alignment and `height * bytesPerRow` fits the buffer, because FrameSink only publishes frames
    /// within its `maxPixels` and sized the buffers for that worst case.
    nonisolated static func makeTexture(for frame: ReadyFrame, in frames: FrameSink) -> (any MTLTexture)? {
        let buffer     = frame.bufferIndex == 0 ? frames.buffers.0 : frames.buffers.1
        let descriptor = MTLTextureDescriptor.texture2DDescriptor(
            pixelFormat : .rgba8Unorm,
            width       : frame.width,
            height      : frame.height,
            mipmapped   : false
        )
        descriptor.storageMode = .shared
        descriptor.usage       = .shaderRead

        return buffer.makeTexture(descriptor: descriptor, offset: 0, bytesPerRow: frame.bytesPerRow)
    }

    // MARK: - Gestures

    /// handlePinch zooms around the fingers; resetting `scale` makes each call a relative step.
    @objc
    private func handlePinch(_ recognizer: UIPinchGestureRecognizer) {
        guard recognizer.state == .began || recognizer.state == .changed else { return }

        canvasTransform.pinch(
            by       : Double(recognizer.scale),
            around   : recognizer.location(in: self),
            viewSize : bounds.size
        )
        recognizer.scale = 1
    }

    /// handlePan moves the image; resetting the translation makes each call a relative step.
    @objc
    private func handlePan(_ recognizer: UIPanGestureRecognizer) {
        guard recognizer.state == .began || recognizer.state == .changed else { return }

        canvasTransform.pan(by: recognizer.translation(in: self))
        recognizer.setTranslation(.zero, in: self)
    }

    @objc
    private func handleDoubleTap(_ recognizer: UITapGestureRecognizer) {
        canvasTransform.reset()
    }

    // MARK: - Setup

    private func configureLayer(device: some MTLDevice) {
        isOpaque        = true
        backgroundColor = .black

        metalLayer.device          = device
        metalLayer.pixelFormat     = Self.drawablePixelFormat
        metalLayer.colorspace      = CGColorSpace(name: CGColorSpace.sRGB)
        metalLayer.framebufferOnly = true
        metalLayer.isOpaque        = true
    }

    private func configureRenderPass() {
        let attachment = renderPass.colorAttachments[0]
        attachment?.loadAction  = .clear
        attachment?.storeAction = .store
        attachment?.clearColor  = MTLClearColor(red: 0, green: 0, blue: 0, alpha: 1)
    }

    private func configureGestures() {
        let pinch     = UIPinchGestureRecognizer(target: self, action: #selector(handlePinch(_:)))
        let pan       = UIPanGestureRecognizer(target: self, action: #selector(handlePan(_:)))
        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(handleDoubleTap(_:)))

        doubleTap.numberOfTapsRequired = 2
        pinch.delegate                 = self
        pan.delegate                   = self

        addGestureRecognizer(pinch)
        addGestureRecognizer(pan)
        addGestureRecognizer(doubleTap)
    }

    private func configureAccessibility() {
        isAccessibilityElement = true
        accessibilityLabel     = String(localized: "Photo")
        accessibilityTraits    = .image
    }

    /// listenForFrames sets the sink's wake-up. It runs on the engine thread, so it only flips an
    /// atomic and, if no hop is pending yet, schedules one asynchronous hop to main. The hop clears
    /// the flag before marking the view, so a frame that lands after it schedules a new hop.
    private func listenForFrames() {
        frames.onFrame = { [weak self, crossThread] in
            let claimed = crossThread.isHopPending.compareExchange(
                expected : false,
                desired  : true,
                ordering : .acquiringAndReleasing
            )

            guard claimed.exchanged else { return }

            DispatchQueue.main.async {
                crossThread.isHopPending.store(false, ordering: .releasing)
                self?.setNeedsRedraw()
            }
        }
    }
}

extension CanvasView: UIGestureRecognizerDelegate {

    /// Pinch and pan run together, so two fingers can zoom and move the image in one gesture.
    func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
    ) -> Bool {
        true
    }
}
