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
/// zoom (from 5 % to 3200 % of the photo's real pixels, below fit included, rubber-banded past
/// both ends), pan with momentum and rubber-banded edges, and a double tap that toggles between
/// fit and 100 % at the tap point. A single tap is handed to `onSingleTap`.
///
/// A mouse or trackpad drags the image one to one with no momentum, and Cmd with a trackpad scroll
/// or a wheel zooms around the pointer. A scroll without Cmd pans through a real UIScrollView
/// (`scrollProxy`), so it has the system's own acceleration, as in Photos; its glide after the
/// fingers lift is the canvas's own momentum spring (see `scrollViewWillEndDragging`).
///
/// The gestures write the transform directly while the fingers are down; when they lift, a
/// CanvasSpring carries it to where it should rest (see CanvasTransform.settled), with the flick's
/// velocity as momentum. The spring runs on the same display link as the redraw, so the link stays
/// awake only while something moves. `onZoomChange` reports the zoom percentage for the editor's pill.
///
/// A CADisplayLink advances animation and requests a drawable only when the canvas is dirty.
/// A dedicated serial queue acquires it without blocking main, then main encodes the newest state.
/// Active input and animation can prepare one drawable after GPU completion, ready for the next
/// dirty tick, so the transform is sampled at that tick even when acquisition finished late.
/// The link pauses while idle; an unused prepared drawable is released on its first idle tick.
/// Temporary failures get three delayed retries; after that only new demand wakes the canvas.
///
/// Above the preview's own resolution (more than one screen pixel per preview pixel) the canvas
/// asks, through `onVisibleWindowChange`, for the visible window of the photo rendered at the zoom
/// on screen, capped at the photo's real pixels (FrameWindow.visible): at most every 100 ms while
/// a gesture or a spring moves the view (a draft), and once at full quality when it rests. It
/// keeps drawing the preview and draws the window on top at its place, so a pan never shows a
/// hole. A window is drawn only while it belongs to the frame the canvas would ask for now (a
/// pinch to another zoom below 100 % hides it until the new one arrives) and only if it is newer
/// than `windowFloor`, which rises on every tick that wants no window and when the photo changes,
/// so a window of an earlier photo or of earlier settings never comes back, even one that lands
/// late. From 100 % the window holds the photo's real pixels and is sampled nearest-neighbour
/// (pixels as squares; at 100 % itself moved onto the screen's pixel grid, so 1:1 is exact), from
/// 800 % with a grid on the pixel edges. The real pixels are measured against the engine's
/// native size of the frame (cropped and straightened), which every frame carries.
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
/// to the screen untouched (see Canvas.metal for the shader side).
///
/// The sink double-buffers each kind of frame, so a GPU pass that samples buffer A while the
/// engine publishes twice would read A as it is rewritten (a torn frame, visual only, gone on the
/// next frame). Two rules keep that rare and visible: at most one command buffer samples the sink
/// at a time (its completion wakes any pending redraw), and the completed handler
/// counts the passes that may have torn, in `possibleTearCount`, so T13 can decide whether a
/// fourth buffer is needed.
final class CanvasView: UIView {

    nonisolated static let vertexFunctionName   = "canvasVertex"
    nonisolated static let fragmentFunctionName = "canvasFragment"

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
    private var cachedTextures: InlineArray<3, CachedTexture?> = [nil, nil, nil]

    /// wantedDrawOnLastTick identifies adjacent dirty ticks for FrameTimingProbe. A brief pause
    /// during acquisition preserves continuity; an actual idle gap is excluded by the tick times.
    private var wantedDrawOnLastTick = false

    /// hasDrawWaitingForGPU preserves continuity when pending demand waits through a long GPU pass.
    private var hasDrawWaitingForGPU = false

    /// readyDrawable holds one drawable prepared during active input or animation. The next tick
    /// consumes it or releases it if no drawing demand remains, even while a finger stays down.
    private var readyDrawable: (result: AcquiredDrawable, token: DrawableRequestState.Token)?

    private var isAnimatingTransform: Bool { spring != nil || !zoomEasing.isIdle }

    /// canAnticipateDraw identifies input that may need another frame after this GPU pass. A
    /// prepared drawable gets one tick to be used; this condition does not keep idle ticks running.
    private var canAnticipateDraw: Bool {
        isAnimatingTransform || isGestureActive || isMeasurementPanning
    }

    private var isInsideDisplayTick = false
    private var recordedTickDrawable: Int?

    private var drawableRequests = DrawableRequestState()
    private var drawableRetry   : Task<Void, Never>?

    /// acquireNextDrawable retains the injected provider's method once, keeping this UIKit type
    /// concrete while its initializer statically checks DrawableAcquiring. Set before frame callbacks.
    private var acquireNextDrawable:
        (@MainActor (@escaping @MainActor @Sendable (AcquiredDrawable?) -> Void) -> Void)?

    private let pinchRecognizer = UIPinchGestureRecognizer()
    private let panRecognizer   = UIPanGestureRecognizer()
    private let holdRecognizer  = UILongPressGestureRecognizer()
    private var isHolding       = false

    /// pointerPanRecognizer is the click-drag pan of a mouse or trackpad, apart from the finger's
    /// so its release carries no momentum; scrollRecognizer is the trackpad scroll and the wheel
    /// with Cmd held (zoom around the pointer).
    private let pointerPanRecognizer = UIPanGestureRecognizer()
    private let scrollRecognizer     = UIPanGestureRecognizer()

    /// scrollProxy is an invisible UIScrollView that turns a scroll without Cmd into a pan. Its pan
    /// recognizer sits on the canvas and hears only scroll events; it never takes a touch (no user
    /// interaction of its own, so hit tests pass through it). Each change of its `contentOffset`
    /// while the fingers scroll moves the image by the same points the other way. Its
    /// content is far larger than any gesture travels, recentred on the middle between gestures.
    private let scrollProxy: UIScrollView

    /// scrollProxyOffset is the proxy's offset already applied to the image, and scrollProxyMoved
    /// when it last changed: the view counts as moving until `windowRequestInterval` after that, so
    /// a scroll and a wheel's animated notch both ask for throttled draft windows.
    private var scrollProxyOffset: CGPoint = .zero
    private var scrollProxyMoved : CFTimeInterval = 0

    /// scrollReleaseVelocity is the image velocity (points per second) of the scroll being released,
    /// from `scrollViewWillEndDragging` to `scrollViewDidEndDragging`.
    private var scrollReleaseVelocity: SIMD2<Double> = .zero

    /// isScrollProxyAnimating is true while the proxy moves on its own without a deceleration: a
    /// wheel notch scrolls as a short animation. It is stopped and settled like a glide.
    private var isScrollProxyAnimating = false

    /// isStoppingScrollGlide is true while `stopScrollGlide` stops the proxy: what it reports then
    /// does not move the image.
    private var isStoppingScrollGlide = false

    /// isScrollProxyGliding is true while the proxy moves after the fingers left: a wheel notch's
    /// animation, or a deceleration UIKit runs despite `scrollViewWillEndDragging` (it is stopped).
    private var isScrollProxyGliding: Bool {
        scrollProxy.isDecelerating || isScrollProxyAnimating
    }

    /// scrollProxyContent is the proxy's content size on each side, in points.
    private static let scrollProxyContent: CGFloat = 1_000_000

    /// holdDuration is how long a still finger must stay down before the before look shows.
    static let holdDuration: TimeInterval = 0.35

    private var spring         : CanvasSpring?
    private var springStartTime : CFTimeInterval = 0
    private var previewGeometry = PreviewGeometry()

    /// lastTickTime is the target presentation time of the last display link update, the frame the
    /// transform on screen belongs to; a new spring starts there (CanvasSpring.startTime). The
    /// frameInterval is the link's refresh, measured between updates while it runs, and
    /// isLinkPausedOnLastTick says the next update comes after a sleep (no interval to measure).
    private var lastTickTime          : CFTimeInterval = 0
    private var frameInterval         : CFTimeInterval = 1.0 / 60
    private var isLinkPausedOnLastTick: Bool = true

    /// onSingleTap runs when the canvas is tapped once (the editor closes its panel with it).
    var onSingleTap: (() -> Void)?

    /// onTouchDown runs the moment a finger lands on the canvas, before any gesture is decided. The editor
    /// closes its card with it, without waiting for the double-tap window, and a hold brings the card back.
    var onTouchDown: (() -> Void)?

    /// onHoldChange runs with true when a press and hold begins (the finger stayed within the usual slop
    /// for `holdDuration`, alone on the canvas) and with false when it ends: the editor shows the
    /// before look for as long as the finger stays down.
    var onHoldChange: ((Bool) -> Void)?

    /// onToggleBefore is VoiceOver's "Show Original" action; `isShowingBefore` names it.
    var onToggleBefore: (() -> Void)?

    /// isShowingBefore names the VoiceOver action for what it does next; the editor sets it. A change
    /// is announced, since VoiceOver reads neither the renamed action nor the capsule on its own.
    var isShowingBefore = false {
        didSet {
            guard isShowingBefore != oldValue else { return }

            let announcement = isShowingBefore
                ? String(localized: "Showing original")
                : String(localized: "Showing edits")

            configureAccessibilityActions()
            UIAccessibility.post(notification: .announcement, argument: announcement)
        }
    }

    /// onZoomChange gets the zoom as a percentage of the photo's real pixels on screen pixels, at
    /// most once per display frame and only when the pill's text (ZoomScale.label) changes.
    var onZoomChange: ((Double) -> Void)?

    /// onVisibleWindowChange gets the window of the zoomed frame to render over the preview (nil
    /// when the preview is enough) and whether a draft will do because the view is moving.
    var onVisibleWindowChange: ((FrameWindow?, _ isDraft: Bool) -> Void)?

    /// isMeasurementPanning makes `measurementPan(at:)` count as a gesture, so its windows are
    /// throttled drafts as under a finger. The launch runner sets it for the pan sweep only.
    var isMeasurementPanning = false {
        didSet {
            if !isMeasurementPanning {
                wakeForWindowUpdate()
            }
        }
    }

    /// photoSize is the open photo's size in real pixels (`.zero` when unknown), as the editor reads
    /// it. The zoom percentage is measured against it until a frame carries the engine's native
    /// size of the frame (see `nativeSize`), not against the smaller preview.
    var photoSize: CGSize = .zero {
        didSet {
            if photoSize != oldValue {
                reportedPercent = nil
                setNeedsRedraw()
            }
        }
    }

    /// clippingOverlay marks the clipped pixels of the preview and the window (Canvas.metal).
    var clippingOverlay: ClippingOverlay = [] {
        didSet {
            if clippingOverlay != oldValue {
                setNeedsRedraw()
            }
        }
    }

    private var reportedPercent: String?

    private lazy var haptics = TickHaptics(view: self)

    /// disarmedLandmark is the landmark zoom that ticked last and may not tick again until the
    /// pinch leaves the band around it (ZoomScale.landmarkTick).
    private var disarmedLandmark: Double?

    /// pinchCenter is the fingers' last centroid, where a zoom stretched past the range springs back.
    private var pinchCenter: CGPoint?

    /// zoomEasing is the zoom a trackpad pinch or Cmd + scroll still owes the display (ZoomEasing),
    /// stepped on each display update from `lastZoomEasingTime`. A gesture that ends while it is
    /// owed settles once it is paid (`settlesAfterZoomEasing`).
    private var zoomEasing             = ZoomEasing()
    private var lastZoomEasingTime     : CFTimeInterval = 0
    private var settlesAfterZoomEasing = false

    /// lastIndirectPinchScale is the trackpad pinch's scale at its last event. That scale counts from
    /// the gesture's start: setting it back to 1, as a finger pinch does, has no effect (F5 recording).
    private var lastIndirectPinchScale = 1.0

    /// stalePinchTimeout is how long a trackpad pinch may go without an event before it counts as one
    /// UIKit will never end (`watchForStalePinch`). The 96 pinches recorded in F5 and F6 that did end
    /// had events at most 50 ms apart; the stale one had gaps of 9.2 and 5.6 s.
    static let stalePinchTimeout: TimeInterval = 0.3

    /// indirectPinchEvents counts the trackpad pinch events, so a stale check knows whether a newer
    /// event came after its own.
    private var indirectPinchEvents = 0

    /// windowRequestInterval is the least time between two window requests while the view moves.
    private static let windowRequestInterval: CFTimeInterval = 0.1

    /// gridPercent is the zoom from which a grid is drawn on the image pixels' edges, and
    /// gridOpacity how far its lines step toward mid grey.
    private static let gridPercent = 800.0
    private static let gridOpacity: Float = 0.3

    /// wantedWindow is the window the canvas needs for the current view (FrameWindow.visible),
    /// recomputed on each tick; sentWindow is the last one handed to `onVisibleWindowChange`.
    private var wantedWindow        : FrameWindow?
    private var sentWindow          : FrameWindow?
    private var isSentWindowDraft   : Bool = false
    private var lastWindowSendTime  : CFTimeInterval = 0
    private var isWindowUpdatePending = false

    /// lastZoomChange is when a tick last saw the zoom differ from `lastTickZoom`. Until the zoom has
    /// held still for `windowRequestInterval` no draft window is asked for: during a pinch, Cmd +
    /// scroll or a zoom spring each would be a new frame size or region the engine renders on the
    /// GPU while the canvas draws, and those renders made the zoom hitch on the iPad. Below 100 %
    /// they were never even drawn (another frame by the time they landed, see `isShowable`).
    private var lastTickZoom  : Double = 1
    private var lastZoomChange: CFTimeInterval = 0

    /// prefetchTarget is the target of the running zoom spring or scroll glide whose window was
    /// asked for when it started (`prefetchWindow`). Until it lands no window, nil included, is
    /// asked for and the floor stays where it is: the ticks below the preview's resolution early in
    /// a fit to 100 % spring would otherwise bury the prefetched window and ask for it again at the
    /// settle.
    private var prefetchTarget: CanvasTransform?

    /// windowFloor is the newest generation published on the last tick that wanted no window (or
    /// at the last photo change): a window at or below it is never drawn again.
    private var windowFloor: UInt64 = 0

    /// previewFloor is the newest generation already published when this canvas was made: a scene
    /// that connects later must not show a frame from before it (another scene's, or the one left
    /// by a photo it never opened), so the first frame drawn is one asked for after.
    private let previewFloor: UInt64

    /// canvasTransform is the gesture state (UIView already owns `transform`). Any real change to
    /// it asks for one redraw. Readable for the tests.
    private(set) var canvasTransform = CanvasTransform() {
        didSet {
            if canvasTransform != oldValue {
                setNeedsRedraw()
            }
        }
    }

    /// isSpringing is true while a spring (a zoom, a glide, a snap back) still moves the transform.
    /// Readable for the tests, which wait on it instead of on a fixed time.
    var isSpringing: Bool {
        spring != nil
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

    /// CanvasStyle mirrors Canvas.metal's struct of the same name: how one quad is sampled. Its
    /// layout (8 + 4 + 4 + 4 bytes, padded to 24 for the float2's alignment) matches the shader's.
    private struct CanvasStyle {

        var gridSize   : SIMD2<Float>
        var gridOpacity: Float
        var nearest    : UInt32
        var clipping   : UInt32
    }

    /// CrossThreadState is what the engine thread and Metal's completion thread touch: only atomics,
    /// so it is Sendable without a lock. It is a separate object so those closures hold it, never the
    /// view: a UIView must not be retained, and possibly freed, off the main thread.
    // Nonisolated: the engine thread and Metal's completion thread use it, the app default is MainActor.
    private nonisolated final class CrossThreadState: Sendable {

        let isHopPending  = Atomic<Bool>(false)
        let possibleTears = Atomic<Int>(0)
        let droppedFrames = Mutex(DroppedFrameRetries())

        /// commandBufferCompleted(sampled:newest:) runs on Metal's completion thread when a pass ends,
        /// with the newest generation the pass sampled and the newest one published by then.
        ///
        /// Any newer generation published while the pass ran is a possible tear. One publish lands
        /// in the free buffer and frees a sampled one as the engine's next write target, so the
        /// engine may already be copying into it while the GPU still reads it; two or more publishes
        /// certainly rewrote it. The count is an upper bound: an exposed pass may still read clean.
        func commandBufferCompleted(sampled: UInt64?, newest: UInt64?) {
            if let sampled,
               let newest,
               newest > sampled {
                possibleTears.add(1, ordering: .relaxed)
            }
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

    /// init(frames:device:scrollProxy:) uses the dedicated acquisition queue for this canvas's layer.
    convenience init(
        frames     : FrameSink,
        device     : some MTLDevice,
        scrollProxy: UIScrollView = UIScrollView()
    ) {
        self.init(
            frames             : frames,
            device             : device,
            scrollProxy        : scrollProxy,
            makeDrawableSource : { DrawableAcquisitionQueue(layer: $0) }
        )
    }

    /// init(frames:device:scrollProxy:makeDrawableSource:) builds the pipeline and wires the sink.
    /// The factory receives the configured backing layer; tests can hold or fail its acquisitions.
    /// `scrollProxy` is the invisible scroll view (see its doc); tests can pass a subclass.
    ///
    /// The pipeline is built here from the app's default library (Canvas.metal), once per view. If
    /// it does not build, the error is logged and the canvas only clears to the surround: the editor
    /// stays usable, and CanvasTextureTests fails on the same error. A device that cannot make a
    /// command queue is out of resources at launch, with nothing to degrade to.
    init<Source: DrawableAcquiring>(
        frames             : FrameSink,
        device             : some MTLDevice,
        scrollProxy        : UIScrollView = UIScrollView(),
        makeDrawableSource : (CAMetalLayer) -> Source
    ) {
        guard let commandQueue = device.makeCommandQueue() else {
            fatalError("CanvasView: the device cannot make a command queue")
        }

        do {
            pipeline = try Self.makePipelineState(
                device      : device,
                pixelFormat : Self.drawablePixelFormat
            )
        } catch {
            Self.logger.error("The canvas shaders did not build: \(String(describing: error), privacy: .public)")
            pipeline = nil
        }

        self.frames       = frames
        self.commandQueue = commandQueue
        self.scrollProxy  = scrollProxy
        self.previewFloor = Self.newestGeneration(frames.published())

        super.init(frame: .zero)

        windowFloor = previewFloor

        configureLayer(device: device)
        acquireNextDrawable = makeDrawableSource(metalLayer).acquire(completion:)
        configureRenderPass()
        configureGestures()
        configureAccessibility()
        listenForFrames()
    }

    /// makePipelineState(device:pixelFormat:) looks the two canvas functions up in the
    /// default library and returns the render pipeline that draws into a drawable of `pixelFormat`.
    /// It throws CanvasShaderError when the library or a named function is missing.
    /// The whole build is the `ShaderCompile` signpost: it runs on main when the canvas is made.
    nonisolated static func makePipelineState(
        device     : some MTLDevice,
        pixelFormat: MTLPixelFormat
    ) throws -> any MTLRenderPipelineState {
        let signpost = Signposts.signposter.beginInterval(
            "ShaderCompile",
            id: Signposts.signposter.makeSignpostID()
        )
        defer { Signposts.signposter.endInterval("ShaderCompile", signpost) }

        guard let library = device.makeDefaultLibrary() else {
            throw CanvasShaderError.missingLibrary
        }

        guard let vertexFunction = library.makeFunction(name: vertexFunctionName) else {
            throw CanvasShaderError.missingFunction(vertexFunctionName)
        }

        guard let fragmentFunction = library.makeFunction(name: fragmentFunctionName) else {
            throw CanvasShaderError.missingFunction(fragmentFunctionName)
        }

        let descriptor = MTLRenderPipelineDescriptor()
        descriptor.vertexFunction                  = vertexFunction
        descriptor.fragmentFunction                = fragmentFunction
        descriptor.colorAttachments[0].pixelFormat = pixelFormat

        return try device.makeRenderPipelineState(descriptor: descriptor)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("CanvasView is built in code, not from a storyboard")
    }

    /// resetZoom returns to fit at once, for a newly opened photo: the old zoom and pan belonged to
    /// the previous one, and so does any window published so far.
    func resetZoom() {
        spring = nil
        canvasTransform.reset()
        previewGeometry.reset()
        windowFloor = Self.newestGeneration(frames.published())
    }

    /// measurementPan(at:) moves the image along a circle through 90 % of its pan range, one turn
    /// every 4 s, `elapsed` seconds in: the pan sweep of the measurements, in place of a finger.
    func measurementPan(at elapsed: Double) {
        var edge = canvasTransform
        edge.offset = SIMD2(1e9, 1e9)

        let limits = edge.settled(viewSize: bounds.size, imageSize: imageSize).offset
        let angle  = 2 * Double.pi * elapsed / 4

        spring = nil
        canvasTransform.offset = limits * SIMD2(cos(angle), sin(angle)) * 0.9
    }

    /// measurementPinch(toPercent:progress:) zooms as a pinch at the view center would, to the zoom
    /// `progress` of the way (in log space) from fit to `percent`: one step of a measured pinch, with
    /// its landmark ticks. Pair it with `isMeasurementPanning`, so its windows are drafts.
    func measurementPinch(toPercent percent: Double, progress: Double) {
        guard let scale = zoomScale else { return }

        let target = exp(log(scale.zoom(forPercent: percent)) * min(max(progress, 0), 1))

        spring = nil
        zoom(
            by     : target / canvasTransform.zoom,
            around : CGPoint(x: bounds.midX, y: bounds.midY),
            scale  : scale
        )
    }

    /// zoomToFit animates back to aspect fit (a jump under Reduce Motion), like a double tap from zoomed in.
    func zoomToFit() {
        move(to: CanvasTransform())
    }

    /// zoomToFill animates to the zoom at which the photo covers the whole view, around the view center.
    func zoomToFill() {
        guard let scale = zoomScale else { return }

        zoomAroundCenter(to: scale.fillZoom(viewSize: bounds.size, imageSize: imageSize), scale: scale)
    }

    /// zoomIn animates to the next preset above the current zoom, around the view center.
    func zoomIn() {
        guard let scale = zoomScale else { return }

        zoomAroundCenter(to: scale.zoomIn(from: restingTransform.zoom), scale: scale)
    }

    /// zoomOut animates to the next preset below the current zoom, around the view center.
    func zoomOut() {
        guard let scale = zoomScale else { return }

        zoomAroundCenter(to: scale.zoomOut(from: restingTransform.zoom), scale: scale)
    }

    /// zoom(toPercent:) animates to that percentage of the photo's real pixels on screen pixels
    /// around the view center, clamped to the zoom range and settled inside the edges. Above the
    /// preview's own resolution the canvas asks for a window of the photo at that zoom.
    func zoom(toPercent percent: Double) {
        guard let scale = zoomScale else { return }

        zoomAroundCenter(to: scale.zoom(forPercent: percent), scale: scale)
    }

    /// setNeedsRedraw marks the canvas dirty: the next display link tick draws once, then the link
    /// pauses. Marks made before the next tick coalesce into that one draw.
    func setNeedsRedraw() {
        hasDrawWaitingForGPU = hasDrawWaitingForGPU || drawableRequests.isRendering
        isDirty = true
        drawableRequests.markDirty()
        drawableRetry?.cancel()
        drawableRetry = nil
        updateDisplayLink()
    }

    // MARK: - View lifecycle

    /// willMove invalidates outstanding drawables before UIKit detaches or reparents the layer.
    override func willMove(toWindow newWindow: UIWindow?) {
        invalidateDrawableSurface(isAttached: false)
        displayLink?.isPaused = true
        super.willMove(toWindow: newWindow)
    }

    /// didMoveToWindow owns the display link. It is invalidated on every move (a link left alive
    /// after the view leaves its window keeps running for nothing) and made again only when there
    /// is a window to draw into.
    override func didMoveToWindow() {
        super.didMoveToWindow()

        displayLink?.invalidate()
        displayLink = nil
        invalidateDrawableSurface(isAttached: window != nil)

        guard let window else { return }

        contentScaleFactor = window.traitCollection.displayScale
        setNeedsLayout()

        let maximumRate = window.windowScene?.screen.maximumFramesPerSecond ?? 60
        frameInterval          = 1 / Double(maximumRate)
        isLinkPausedOnLastTick = true

        let link = CADisplayLink(target: self, selector: #selector(displayLinkNeedsUpdate(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(
            minimum   : Float(min(60, maximumRate)),
            maximum   : Float(maximumRate),
            preferred : Float(maximumRate)
        )
        link.isPaused = true
        link.add(to: .main, forMode: .common)

        displayLink = link
        setNeedsRedraw()
    }

    /// layoutSubviews records the desired drawable size; a new size needs one redraw. The layer
    /// receives that size only once any previous acquisition and GPU pass have returned.
    ///
    /// The comparison is against the last size laid out here, not `metalLayer.drawableSize`, so
    /// the redraw does not depend on whether CAMetalLayer ever resizes its drawable by itself.
    override func layoutSubviews() {
        super.layoutSubviews()

        let pixelSize = CGSize(
            width  : (bounds.width  * contentScaleFactor).rounded(.up),
            height : (bounds.height * contentScaleFactor).rounded(.up)
        )

        guard pixelSize != laidOutSize else { return }

        laidOutSize       = pixelSize
        scrollProxy.frame = bounds
        invalidateDrawableSurface(isAttached: window != nil)
        setNeedsRedraw()
    }

    // MARK: - Drawing

    /// displayLinkNeedsUpdate advances input animation at the screen's cadence. Acquiring a
    /// drawable never waits on this run loop; the asynchronous handoff encodes the latest state.
    @objc
    private func displayLinkNeedsUpdate(_ link: CADisplayLink) {
        let recorder = InputRecorder.shared
        let began    = recorder.now
        isInsideDisplayTick = true
        recordedTickDrawable = nil
        let outcome  = FrameTimingProbe.shared.measure("Tick") {
            drawIfDirty(link, recordedTick: began)
        }
        isInsideDisplayTick = false

        recorder.update(
            began          : began,
            target         : link.targetTimestamp,
            zoom           : canvasTransform.zoom,
            drawable       : recordedTickDrawable,
            windowInFlight : began > 0 && isWindowInFlight,
            proxy          : scrollProxy.contentOffset,
            offset         : canvasTransform.offset,
            isGliding      : isScrollProxyGliding
        )

        FrameTimingProbe.shared.tick(
            at          : link.targetTimestamp,
            isPaused    : link.isPaused,
            skippedDraw : outcome.skippedForGPU
        )
        wantedDrawOnLastTick = outcome.wantedDraw

        let time = link.targetTimestamp

        if !isLinkPausedOnLastTick {
            frameInterval = time - lastTickTime
        }

        lastTickTime           = time
        isLinkPausedOnLastTick = link.isPaused
    }

    /// drawIfDirty advances animation and requests one draw of the latest state when needed.
    private func drawIfDirty(
        _ link       : CADisplayLink,
        recordedTick : CFTimeInterval
    ) -> (wantedDraw: Bool, skippedForGPU: Bool) {
        advanceSpring(to: link.targetTimestamp)
        advanceZoomEasing(to: link.targetTimestamp)
        reportZoom()
        updateVisibleWindow(at: CACurrentMediaTime())

        let wanted         = isDirty
        let skippedForGPU = wanted && drawableRequests.isRendering
        let isAnimating    = spring != nil || !zoomEasing.isIdle
        let isContinuation = hasDrawWaitingForGPU || (wantedDrawOnLastTick
            && (isAnimating || link.targetTimestamp - lastTickTime <= frameInterval * 1.5))
        if !wanted {
            discardReadyDrawable()
        }
        if isDirty, let ready = readyDrawable {
            readyDrawable = nil
            if drawableRequests.beginRendering(ready.token) {
                encodeReceivedDrawable(
                    ready.result,
                    token          : ready.token,
                    recordedTick   : recordedTick,
                    isContinuation : isContinuation
                )
            }
        } else {
            requestDrawableIfNeeded(recordedTick: recordedTick, isContinuation: isContinuation)
        }
        updateDisplayLink()
        return (wanted, skippedForGPU)
    }

    /// updateDisplayLink sleeps during an acquisition or GPU pass unless animation still needs
    /// ticks. A prepared drawable gets one tick to be consumed or discarded when input is idle.
    private func updateDisplayLink() {
        if !isDirty && !canAnticipateDraw {
            discardReadyDrawable()
        }
        let canDraw = isDirty && (drawableRequests.canAcquire || readyDrawable != nil)
            && laidOutSize.width > 0 && laidOutSize.height > 0
        let needsTicks = spring != nil || !zoomEasing.isIdle || isWindowUpdatePending
            || readyDrawable != nil
        displayLink?.isPaused = window == nil || (!canDraw && !needsTicks)

        if displayLink?.isPaused == true {
            isLinkPausedOnLastTick = true
        }
    }

    private func invalidateDrawableSurface(isAttached: Bool) {
        discardReadyDrawable()
        drawableRequests.invalidate(isAttached: isAttached)
        wantedDrawOnLastTick = false
        hasDrawWaitingForGPU = false
        drawableRetry?.cancel()
        drawableRetry = nil
    }

    /// discardReadyDrawable drops an unused drawable at idle or before changing the surface.
    private func discardReadyDrawable() {
        guard let ready = readyDrawable else { return }
        readyDrawable = nil
        drawableRequests.discardReady(ready.token)
    }

    /// requestDrawableIfNeeded reserves the whole acquisition and GPU pass before leaving main.
    /// Resizing happens only here, when no previous call or pass still owns the layer's drawable.
    private func requestDrawableIfNeeded(
        recordedTick   : CFTimeInterval,
        isContinuation : Bool,
        isAhead        : Bool = false
    ) {
        guard isDirty || isAhead,
              laidOutSize.width > 0,
              laidOutSize.height > 0,
              let acquireNextDrawable,
              let token = drawableRequests.beginAcquisition()
        else { return }

        let size = laidOutSize
        if !isAhead {
            hasDrawWaitingForGPU = false
        }
        if metalLayer.drawableSize != size {
            metalLayer.drawableSize = size
        }
        acquireNextDrawable { [weak self] result in
            self?.receivedDrawable(
                result,
                token          : token,
                size           : size,
                recordedTick   : recordedTick,
                isContinuation : isContinuation,
                isAhead        : isAhead
            )
        }
    }

    /// receivedDrawable rejects an old surface. An ahead acquisition waits for the next dirty
    /// CAD tick, which samples the latest transform before encoding. An ordinary acquisition
    /// completes the draw already requested by its tick.
    private func receivedDrawable(
        _ result       : AcquiredDrawable?,
        token          : DrawableRequestState.Token,
        size           : CGSize,
        recordedTick   : CFTimeInterval,
        isContinuation : Bool,
        isAhead        : Bool
    ) {
        guard drawableRequests.receive(token, hasDrawable: result != nil, waitForTick: isAhead),
              let result else {
            scheduleDrawableRetry()
            updateDisplayLink()
            return
        }

        let drawable = result.drawable
        guard window != nil,
              size == laidOutSize,
              drawable.texture.width == Int(size.width),
              drawable.texture.height == Int(size.height)
        else {
            if isAhead {
                drawableRequests.discardReady(token)
            } else {
                drawableRequests.finish(token, succeeded: false)
                scheduleDrawableRetry()
            }
            updateDisplayLink()
            return
        }

        if isAhead {
            readyDrawable = (result, token)
            updateDisplayLink()
            return
        }
        encodeReceivedDrawable(
            result,
            token          : token,
            recordedTick   : recordedTick,
            isContinuation : isContinuation
        )
    }

    /// encodeReceivedDrawable uses only the latest transform and frame. Dirty is cleared before
    /// reading the sink, so a later publication schedules another draw. InputRecorder associates
    /// inline encoding with the current tick and asynchronous encoding with the tick that waited.
    private func encodeReceivedDrawable(
        _ result       : AcquiredDrawable,
        token          : DrawableRequestState.Token,
        recordedTick   : CFTimeInterval,
        isContinuation : Bool
    ) {
        let drawable = result.drawable
        hasDrawWaitingForGPU = false
        isDirty = false
        let began   = InputRecorder.shared.now
        let encoded = FrameTimingProbe.shared.measure("Encode") {
            encodeFrame(into: drawable, token: token, isContinuation: isContinuation)
        }

        if encoded, isInsideDisplayTick {
            recordedTickDrawable = drawable.drawableID
        } else if encoded {
            InputRecorder.shared.encoded(
                drawable     : drawable.drawableID,
                updateBegan  : recordedTick,
                encodeBegan  : began,
                zoom         : canvasTransform.zoom,
                offset       : canvasTransform.offset
            )
        } else {
            isDirty = true
            drawableRequests.finish(token, succeeded: false)
            scheduleDrawableRetry()
        }
        updateDisplayLink()
    }

    /// scheduleDrawableRetry gives transient failures a finite recovery window, even after the
    /// last input. Cancellation and the state token both reject old windows and superseded demand.
    private func scheduleDrawableRetry() {
        guard isDirty || isAnimatingTransform,
              let retry = drawableRequests.retry else { return }

        drawableRetry?.cancel()
        drawableRetry = Task { [weak self] in
            do {
                try await Task.sleep(for: .seconds(retry.delay))
            } catch {
                return
            }
            guard let self, self.drawableRequests.resumeRetry(retry) else { return }

            self.drawableRetry = nil
            self.updateDisplayLink()
        }
    }

    /// completedFrame releases the reservation on main, then prepares one drawable if input or
    /// animation is still active. Otherwise it only wakes demand coalesced during the GPU pass.
    private func completedFrame(_ token: DrawableRequestState.Token) {
        guard drawableRequests.finish(token) else { return }

        if canAnticipateDraw {
            requestDrawableIfNeeded(recordedTick: 0, isContinuation: false, isAhead: true)
        }
        updateDisplayLink()
    }

    /// encodeFrame clears `drawable` and draws the sink's newest preview into it, then the window
    /// over it when it may be shown (see the type doc), returning false when nothing was committed
    /// (no command buffer).
    ///
    /// `frames.published()` is read exactly once, here, and the textures and the tear check come
    /// from that one snapshot, so the pass never mixes the geometry of one frame with another.
    // any: Metal vends drawables only as protocol existentials.
    private func encodeFrame(
        into drawable  : any CAMetalDrawable,
        token          : DrawableRequestState.Token,
        isContinuation : Bool
    ) -> Bool {
        guard let commandBuffer = commandQueue.makeCommandBuffer() else { return false }

        let published = frames.published()
        let frame     = published.frame.flatMap { isCurrent($0) ? $0 : nil }

        // The encoder copies the descriptor, so the drawable is not kept past this pass.
        renderPass.colorAttachments[0].texture = drawable.texture
        let encoder = commandBuffer.makeRenderCommandEncoder(descriptor: renderPass)
        renderPass.colorAttachments[0].texture = nil

        guard let encoder else { return false }

        var sampledFrame : ReadyFrame? = nil
        var sampledWindow: ReadyFrame? = nil

        if let frame {
            previewGeometry.observe(frame)
        }

        if let frame, let pipeline, let texture = texture(for: frame) {
            let quad    = canvasTransform.quad(
                viewSize  : bounds.size,
                imageSize : frame.displaySize
            )
            let percent = zoomScale?.percent(forZoom: canvasTransform.zoom) ?? 0

            encoder.setRenderPipelineState(pipeline)
            draw(
                texture,
                in      : quad,
                style   : previewStyle(percent: percent),
                encoder : encoder
            )
            sampledFrame = frame

            if let window = published.window,
               let placement = window.window,
               isShowable(window),
               let windowTexture = self.texture(for: window) {
                let windowQuad   = placement.quad(in: quad)
                let isOneToOne   = window.hasIdentityRaster
                    && abs(percent - 100) <= 100 * FrameWindow.realPixelsTolerance
                let drawableSize = SIMD2<Float>(Float(laidOutSize.width), Float(laidOutSize.height))

                draw(
                    windowTexture,
                    in      : isOneToOne ? Self.pixelAligned(windowQuad, drawableSize: drawableSize) : windowQuad,
                    style   : windowStyle(
                        for      : window,
                        placement: placement,
                        percent  : percent
                    ),
                    encoder : encoder
                )
                sampledWindow = window
            }
        }

        encoder.endEncoding()

        let sampledGeneration = sampledWindow?.generation ?? sampledFrame?.generation

        // Added before present, as Metal requires: the handler ends the frames' signposts.
        // The simulator's Metal has no presented handler, so its frame intervals never end.
        #if !targetEnvironment(simulator)
        if let sampledFrame {
            drawable.addPresentedHandler(Self.presentedHandler(
                generation       : sampledFrame.generation,
                windowGeneration : sampledWindow?.generation,
                isContinuation   : isContinuation,
                crossThread      : crossThread,
                drawAgain        : { [weak self] in
                    DispatchQueue.main.async {
                        guard let self, self.drawableRequests.isCurrent(token) else { return }
                        self.setNeedsRedraw()
                    }
                }
            ))
        }
        #endif

        #if DEBUG
        let committedAt = CACurrentMediaTime()

        commandBuffer.addCompletedHandler { buffer in
            FrameTimingProbe.shared.gpuPass(
                committedAt : committedAt,
                start       : buffer.gpuStartTime,
                end         : buffer.gpuEndTime
            )
        }
        #endif

        if CanvasGPUTime.isActive {
            commandBuffer.addCompletedHandler { buffer in
                CanvasGPUTime.record(start: buffer.gpuStartTime, end: buffer.gpuEndTime)
            }
        }

        commandBuffer.present(drawable)
        commandBuffer.addCompletedHandler { [weak self, crossThread, frames] _ in
            crossThread.commandBufferCompleted(
                sampled : sampledGeneration,
                newest  : Self.newestGeneration(frames.published())
            )
            DispatchQueue.main.async { self?.completedFrame(token) }
        }
        commandBuffer.commit()

        return true
    }

    /// draw encodes one textured quad (NDC corners) sampled with `style`.
    // any: Metal vends textures and encoders only as protocol existentials.
    private func draw(
        _ texture: any MTLTexture,
        in quad  : (min: SIMD2<Float>, max: SIMD2<Float>),
        style    : CanvasStyle,
        encoder  : any MTLRenderCommandEncoder
    ) {
        var corners = SIMD4<Float>(lowHalf: quad.min, highHalf: quad.max)
        var style   = style

        encoder.setVertexBytes(&corners, length: MemoryLayout<SIMD4<Float>>.stride, index: 0)
        encoder.setFragmentBytes(&style, length: MemoryLayout<CanvasStyle>.stride, index: 0)
        encoder.setFragmentTexture(texture, index: 0)
        encoder.drawPrimitives(type: .triangleStrip, vertexStart: 0, vertexCount: 4)
    }

    /// previewStyle samples the preview bilinearly. From `gridPercent` its grid follows the photo's
    /// real pixels (the frame of the window the canvas wants), so it lines up with the window's.
    private func previewStyle(percent: Double) -> CanvasStyle {
        let frame = wantedWindow.map { SIMD2<Float>(Float($0.frameWidth), Float($0.frameHeight)) }

        return CanvasStyle(
            gridSize    : frame ?? .one,
            gridOpacity : percent >= Self.gridPercent && frame != nil ? Self.gridOpacity : 0,
            nearest     : 0,
            clipping    : clippingOverlay.rawValue
        )
    }

    /// windowStyle uses nearest sampling at native zoom only for an identity raster. Reduced
    /// drafts stay bilinear; their grid still follows the logical photo pixels.
    private func windowStyle(
        for frame: ReadyFrame,
        placement: FrameWindow,
        percent  : Double
    ) -> CanvasStyle {
        let usesNearest = frame.hasIdentityRaster
            && percent >= 100 * (1 - FrameWindow.realPixelsTolerance)

        return CanvasStyle(
            gridSize    : SIMD2<Float>(Float(placement.width), Float(placement.height)),
            gridOpacity : percent >= Self.gridPercent ? Self.gridOpacity : 0,
            nearest     : usesNearest ? 1 : 0,
            clipping    : clippingOverlay.rawValue
        )
    }

    /// pixelAligned(_:drawableSize:) moves `quad` (NDC corners) by less than half a pixel, keeping
    /// its size, so its corner lands on a pixel edge of a `drawableSize` drawable. At 100 % a window
    /// texel is one pixel wide, so each pixel then samples exactly one texel: nearest sampling with
    /// a sub-pixel pan offset would pick its texels unevenly, a column now and then twice.
    nonisolated static func pixelAligned(
        _ quad      : (min: SIMD2<Float>, max: SIMD2<Float>),
        drawableSize: SIMD2<Float>
    ) -> (min: SIMD2<Float>, max: SIMD2<Float>) {
        let half  = drawableSize / 2
        let pixel = (quad.min + 1) * half
        let shift = (pixel.rounded(.toNearestOrAwayFromZero) - pixel) / half

        return (min: quad.min + shift, max: quad.max + shift)
    }

    /// isCurrent tells whether a published preview is one this canvas may draw (see `previewFloor`).
    func isCurrent(_ preview: ReadyFrame) -> Bool {
        preview.generation > previewFloor
    }

    /// isShowable tells whether a published window still belongs on screen: newer than the floor
    /// and of the frame the canvas would ask for now (see the type doc). Internal for the tests.
    func isShowable(_ window: ReadyFrame) -> Bool {
        guard let placement = window.window, let wantedWindow else { return false }

        return window.generation > windowFloor && placement.hasSameFrame(as: wantedWindow)
    }

    /// newestGeneration is the newest generation among the published frames, 0 before any.
    private nonisolated static func newestGeneration(_ published: (frame: ReadyFrame?, window: ReadyFrame?)) -> UInt64 {
        max(published.frame?.generation ?? 0, published.window?.generation ?? 0)
    }

    #if !targetEnvironment(simulator)
    /// presentedHandler ends the signposts waiting for the drawn preview's `generation`, then for the
    /// window drawn over it, once the drawable has left the display pipeline. It is built here,
    /// nonisolated, because Metal calls it on its own thread.
    ///
    /// A presented time of 0 means the frame was not shown, not that the measurement is wrong: a
    /// probe on the iPad (C1) saw every drawable drawn while the editor was still fading in report 0,
    /// and the first frame after an open can land there. The signposts stay open and `drawAgain`
    /// asks for the same frame on the next tick, until a drawable is presented (`DroppedFrameRetries`
    /// bounds it for a canvas that stays hidden, then the interval ends as `dropped`).
    private nonisolated static func presentedHandler(
        generation      : UInt64,
        windowGeneration: UInt64?,
        isContinuation  : Bool,
        crossThread     : CrossThreadState,
        drawAgain       : @escaping @Sendable () -> Void
    ) -> @Sendable (any MTLDrawable) -> Void {
        { drawable in
            let isDropped = drawable.presentedTime == 0

            FrameTimingProbe.shared.presented(at: drawable.presentedTime, isContinuation: isContinuation)
            InputRecorder.shared.presented(drawable: drawable.drawableID, at: drawable.presentedTime)

            let isDrawnAgain = crossThread.droppedFrames.withLock { retries in
                retries.shouldDrawAgain(isDropped: isDropped, at: .now)
            }

            if isDrawnAgain {
                drawAgain()
                return
            }

            Signposts.framePresented(generation: generation, isDropped: isDropped)

            if let windowGeneration {
                Signposts.framePresented(generation: windowGeneration, isDropped: isDropped)
            }
        }
    }
    #endif

    /// texture(for:) returns the cached texture over the buffer holding `frame`, making a new one
    /// only when that buffer's frame changed size or stride.
    private func texture(for frame: ReadyFrame) -> (any MTLTexture)? {
        if let cached = cachedTextures[frame.bufferIndex], cached.fits(frame) {
            return cached.texture
        }

        let made = FrameTimingProbe.shared.measure("MakeTexture") { Self.makeTexture(for: frame, in: frames) }

        guard let texture = made else {
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
        let buffer     = frames.buffer(at: frame.bufferIndex)
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

    /// handlePinch zooms around the fingers; resetting `scale` makes each call a relative step. A
    /// trackpad pinch's scale is cumulative, so its step is the ratio to the last one, damped
    /// (ZoomScale.indirectPinchFactor) and eased (`easeZoom`); fit and 100 % tick (TickHaptics).
    @objc
    private func handlePinch(_ recognizer: UIPinchGestureRecognizer) {
        let began = InputRecorder.shared.now

        // A trackpad pinch arrives as a transform event, with no touches.
        let isIndirect = recognizer.numberOfTouches == 0
        let raw        = Double(recognizer.scale)
        var factor     = 1.0

        switch recognizer.state {
            case .began, .changed:
                spring = nil
                stopScrollGlide()

                if isIndirect {
                    watchForStalePinch()
                }

                guard let scale = zoomScale else { return }

                if isIndirect {
                    if recognizer.state == .began {
                        lastIndirectPinchScale = 1
                    }

                    factor                 = ZoomScale.indirectPinchFactor(for: raw / lastIndirectPinchScale)
                    lastIndirectPinchScale = raw
                    easeZoom(by: factor, around: recognizer.location(in: self))
                } else {
                    factor = raw
                    zoomEasing.reset()
                    zoom(
                        by     : factor,
                        around : recognizer.location(in: self),
                        scale  : scale
                    )
                    recognizer.scale = 1
                }

            case .ended, .cancelled, .failed:
                settleAfterZoomEasing()

            default:
                break
        }

        InputRecorder.shared.event(
            "pinch",
            state      : recognizer.state,
            rawX       : raw,
            factor     : factor,
            zoom       : canvasTransform.zoom,
            isIndirect : isIndirect,
            began      : began
        )
    }

    /// zoom(by:around:scale:) is one zoom step of a pinch or of Cmd + scroll: it zooms about `center`
    /// within the range, remembers the center for the spring back, and ticks at fit and 100 %.
    private func zoom(by factor: Double, around center: CGPoint, scale: ZoomScale) {
        let zoomBefore = canvasTransform.zoom

        canvasTransform.pinch(
            by       : factor,
            around   : center,
            range    : scale.range,
            viewSize : bounds.size
        )
        pinchCenter = center

        let tick = scale.landmarkTick(
            from     : zoomBefore,
            to       : canvasTransform.zoom,
            disarmed : disarmedLandmark
        )
        disarmedLandmark = tick.disarmed

        if tick.ticks {
            FrameTimingProbe.shared.measure("Haptic") { haptics.tick(at: center) }
        }

        FrameTimingProbe.shared.measure("HapticPrepare") { haptics.prepare() }
    }

    /// easeZoom(by:around:) owes the display a trackpad zoom step about `center`, shown over the
    /// next display updates (ZoomEasing); under Reduce Motion it zooms at once. It stops a scroll
    /// glide first, so the pan and the zoom never fight. Internal for the tests.
    func easeZoom(by factor: Double, around center: CGPoint) {
        spring = nil
        stopScrollGlide()

        guard let scale = zoomScale else { return }

        guard !UIAccessibility.isReduceMotionEnabled else {
            zoom(by: factor, around: center, scale: scale)
            return
        }

        if zoomEasing.isIdle {
            lastZoomEasingTime = CACurrentMediaTime()
        }

        zoomEasing.add(factor, around: center)
        settlesAfterZoomEasing = false
        displayLink?.isPaused  = false
    }

    /// advanceZoomEasing shows the part of the owed zoom due by `time`, through `zoom(by:around:scale:)`
    /// so the landmark ticks and the windows follow the zoom on screen, and settles a gesture that
    /// ended meanwhile once nothing is owed.
    private func advanceZoomEasing(to time: CFTimeInterval) {
        guard !zoomEasing.isIdle else { return }

        guard let scale = zoomScale else {
            zoomEasing.reset()
            settlesAfterZoomEasing = false
            return
        }

        let factor = zoomEasing.step(elapsed: time - lastZoomEasingTime)
        lastZoomEasingTime = time

        zoom(by: factor, around: zoomEasing.anchor, scale: scale)

        if zoomEasing.isIdle, settlesAfterZoomEasing {
            settlesAfterZoomEasing = false
            settle(velocity: .zero)
        }
    }

    /// settleAfterZoomEasing settles a trackpad zoom that ended now, or once its owed zoom is shown.
    private func settleAfterZoomEasing() {
        if zoomEasing.isIdle {
            settle(velocity: .zero)
        } else {
            settlesAfterZoomEasing = true
        }
    }

    /// watchForStalePinch checks once, `stalePinchTimeout` after this trackpad pinch event, whether a
    /// newer one came. If none did, the pinch is cancelled: UIKit can leave a trackpad pinch `changed`
    /// with no end (F6's edge session, 15 s), and while it counts as a gesture in progress no release
    /// glides, nothing springs back from the rubber band, every window is a draft and the next pinch
    /// starts from its leftover scale. A trackpad pinch held perfectly still that long ends too.
    private func watchForStalePinch() {
        indirectPinchEvents &+= 1

        let event = indirectPinchEvents

        DispatchQueue.main.asyncAfter(deadline: .now() + Self.stalePinchTimeout) { [weak self] in
            guard let self, indirectPinchEvents == event else { return }

            cancelStaleTrackpadPinch()
        }
    }

    /// cancelStaleTrackpadPinch cancels a trackpad pinch (no touches) still `began` or `changed`.
    /// Turning the recognizer off and on ends it as `cancelled`, which `handlePinch` settles.
    private func cancelStaleTrackpadPinch() {
        let isPinching = pinchRecognizer.state == .began || pinchRecognizer.state == .changed

        guard isPinching, pinchRecognizer.numberOfTouches == 0 else { return }

        pinchRecognizer.isEnabled = false
        pinchRecognizer.isEnabled = true
    }

    /// handlePan moves the image, rubber-banded past the edges; resetting the translation makes
    /// each call a relative step. On release a finger's flick becomes momentum, a pointer's does not.
    @objc
    private func handlePan(_ recognizer: UIPanGestureRecognizer) {
        switch recognizer.state {
            case .began, .changed:
                spring = nil
                stopScrollGlide()
                canvasTransform.pan(
                    by        : recognizer.translation(in: self),
                    viewSize  : bounds.size,
                    imageSize : imageSize
                )
                recognizer.setTranslation(.zero, in: self)

            case .ended:
                let velocity = recognizer.velocity(in: self)
                let release  = CanvasSpring.releaseVelocity(
                    SIMD2<Double>(Double(velocity.x), Double(velocity.y)),
                    isPointer: recognizer === pointerPanRecognizer
                )

                settle(velocity: release)

            case .cancelled, .failed:
                settle(velocity: .zero)

            default:
                break
        }
    }

    /// handleScroll zooms around the pointer with the vertical travel of a trackpad scroll or a
    /// wheel while Cmd is held; it begins only then (see `gestureRecognizerShouldBegin`), and a
    /// scroll without Cmd pans through `scrollProxy`. A zoom never glides.
    @objc
    private func handleScroll(_ recognizer: UIPanGestureRecognizer) {
        let began       = InputRecorder.shared.now
        let translation = recognizer.translation(in: self)
        var factor      = 1.0

        switch recognizer.state {
            case .began, .changed:
                recognizer.setTranslation(.zero, in: self)

                factor = ZoomScale.scrollZoomFactor(forTranslation: Double(translation.y))
                easeZoom(by: factor, around: recognizer.location(in: self))

            case .ended, .cancelled, .failed:
                settleAfterZoomEasing()

            default:
                break
        }

        InputRecorder.shared.event(
            "cmd-scroll",
            state      : recognizer.state,
            rawX       : Double(translation.x),
            rawY       : Double(translation.y),
            factor     : factor,
            zoom       : canvasTransform.zoom,
            isIndirect : recognizer.numberOfTouches == 0,
            began      : began
        )
    }

    /// handleDoubleTap toggles fit and the photo's real 100 % around the tap (see
    /// CanvasTransform.toggledZoom).
    /// With Reduce Motion the zoom jumps instead of animating.
    @objc
    private func handleDoubleTap(_ recognizer: UITapGestureRecognizer) {
        let target = canvasTransform.toggledZoom(
            at          : recognizer.location(in: self),
            viewSize    : bounds.size,
            imageSize   : imageSize,
            screenScale : Double(contentScaleFactor),
            photoScale  : CanvasTransform.photoScale(previewSize: imageSize, photoSize: nativeSize)
        )

        move(to: target)
    }

    /// move goes to `target` with the snap spring, or at once under Reduce Motion.
    private func move(to target: CanvasTransform) {
        zoomEasing.reset()

        if UIAccessibility.isReduceMotionEnabled {
            spring          = nil
            canvasTransform = target
        } else {
            animate(to: target, velocity: .zero, frequency: CanvasSpring.snapFrequency)
        }
    }

    @objc
    private func handleSingleTap(_ recognizer: UITapGestureRecognizer) {
        onSingleTap?()
    }

    /// A finger landing on the canvas catches a gliding image, as in a scroll view, and tells
    /// `onTouchDown` at once so the editor can close its card.
    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) {
        super.touchesBegan(touches, with: event)

        spring = nil
        stopScrollGlide()
        onTouchDown?()
    }

    /// A touch that ends without becoming a gesture still lets a caught image settle.
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) {
        super.touchesEnded(touches, with: event)

        if spring == nil {
            settle(velocity: .zero)
        }
    }

    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) {
        super.touchesCancelled(touches, with: event)

        if spring == nil {
            settle(velocity: .zero)
        }
    }

    /// A hold never begins while a pinch or a pan moves the image, nor on a canvas with no frame yet.
    /// A scroll with Cmd held zooms (scrollRecognizer), one without pans (the proxy's recognizer).
    override func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
        let isCommand = gestureRecognizer.modifierFlags.contains(.command)

        switch gestureRecognizer {
            case holdRecognizer:
                return !isGestureActive && frames.latest() != nil

            case scrollRecognizer:
                return Self.acceptsScroll(fromProxy: false, withCommand: isCommand)

            case scrollProxy.panGestureRecognizer:
                return Self.acceptsScroll(fromProxy: true, withCommand: isCommand)

            default:
                return super.gestureRecognizerShouldBegin(gestureRecognizer)
        }
    }

    // MARK: - Motion

    /// imageSize is the size of the photo on screen, in preview pixels: the last full frame's,
    /// because drafts may be smaller, or the newest frame's before any full frame arrived.
    private var imageSize: CGSize {
        previewGeometry.imageSize(for: frames.latest())
    }

    /// restingTransform is where the canvas is going: the running spring's target, or the current one.
    private var restingTransform: CanvasTransform {
        spring?.target ?? canvasTransform
    }

    /// nativeSize is what 100 % measures against: the whole frame at the photo's own resolution,
    /// cropped and straightened, as the newest frame reports it (the same for drafts), else the
    /// editor's `photoSize` (a frame without it, or none yet).
    private var nativeSize: CGSize {
        guard let frame = frames.latest(), frame.nativeWidth > 0, frame.nativeHeight > 0 else { return photoSize }

        return CGSize(width: frame.nativeWidth, height: frame.nativeHeight)
    }

    /// zoomScale is the zoom-to-percent conversion for the current view, photo and screen, or nil
    /// while any of them is unknown.
    private var zoomScale: ZoomScale? {
        ZoomScale(
            viewSize    : bounds.size,
            imageSize   : imageSize,
            screenScale : Double(contentScaleFactor),
            photoScale  : CanvasTransform.photoScale(previewSize: imageSize, photoSize: nativeSize)
        )
    }

    /// zoomAroundCenter moves to `zoom` keeping the view center fixed, settled inside the edges.
    private func zoomAroundCenter(to zoom: Double, scale: ZoomScale) {
        var target = restingTransform

        target.pinch(
            by       : zoom / target.zoom,
            around   : CGPoint(x: bounds.midX, y: bounds.midY),
            range    : scale.range,
            viewSize : bounds.size
        )

        move(to: target.settled(viewSize: bounds.size, imageSize: imageSize))
    }

    private var isGestureActive: Bool {
        let recognizers: [UIGestureRecognizer] = [
            pinchRecognizer,
            panRecognizer,
            pointerPanRecognizer,
            scrollRecognizer,
            scrollProxy.panGestureRecognizer,
        ]

        return recognizers.contains { $0.state == .began || $0.state == .changed }
    }

    /// settle springs the transform to where it should rest once no finger moves it. A flick
    /// (above 60 pt/s) glides with momentum toward its projected point, clamped to the edges, with
    /// the velocity capped so it comes to rest on an edge, never past it (momentumVelocity);
    /// anything else snaps back quickly. A scroll glide (`isScrollGlide`) asks for its resting
    /// window once, at the release, and none on the way. A zoom resting within
    /// ZoomScale.realPixelsSnap of the real 100 % lands on it exactly (at once under Reduce Motion),
    /// where the window shows the photo 1:1 with nearest sampling: a scale a hair off 1 would
    /// double or skip a column now and then.
    private func settle(velocity: SIMD2<Double>, isScrollGlide: Bool = false) {
        guard !isGestureActive else { return }

        // The view rests where it is or springs there: either way the window catches up at rest.
        wakeForWindowUpdate()

        let isFlick = (velocity * velocity).sum() > 60 * 60
        var target  = canvasTransform

        if isFlick {
            target.offset += velocity * CanvasSpring.momentumProjection
        }

        var isSnappedToRealPixels = false

        if let scale = zoomScale {
            let center = pinchCenter ?? CGPoint(x: bounds.midX, y: bounds.midY)

            target = target.settled(
                viewSize  : bounds.size,
                imageSize : imageSize,
                range     : scale.range,
                around    : center
            )

            let resting = scale.restingZoom(for: target.zoom)

            if resting != target.zoom {
                target.pinch(
                    by       : resting / target.zoom,
                    around   : center,
                    range    : scale.range,
                    viewSize : bounds.size
                )
                target                = target.settled(viewSize: bounds.size, imageSize: imageSize)
                isSnappedToRealPixels = true
            }
        } else {
            target = target.settled(viewSize: bounds.size, imageSize: imageSize)
        }

        guard target != canvasTransform else { return }

        if isSnappedToRealPixels, UIAccessibility.isReduceMotionEnabled {
            spring          = nil
            canvasTransform = target
            return
        }

        // A zoom springing back from past a limit snaps; momentum is for a pan alone.
        var momentum = SIMD2<Double>.zero

        if isFlick, target.zoom == canvasTransform.zoom {
            momentum = CanvasSpring.momentumVelocity(
                velocity,
                from : canvasTransform.offset,
                to   : target.offset
            )
        }

        let isMomentum = momentum != .zero

        animate(
            to            : target,
            velocity      : momentum,
            frequency     : isMomentum ? CanvasSpring.momentumFrequency : CanvasSpring.snapFrequency,
            prefetchesPan : isScrollGlide
        )
    }

    /// animate starts the spring to `target`. `prefetchesPan` asks for the resting window of a spring
    /// that only pans too (a scroll glide), so none is asked on the way.
    private func animate(
        to target     : CanvasTransform,
        velocity      : SIMD2<Double>,
        frequency     : Double,
        prefetchesPan : Bool = false
    ) {
        zoomEasing.reset()
        settlesAfterZoomEasing = false
        spring = CanvasSpring(
            from           : canvasTransform,
            to             : target,
            offsetVelocity : velocity,
            frequency      : frequency
        )
        springStartTime = 0
        displayLink?.isPaused = false

        prefetchWindow(for: target, evenIfPanning: prefetchesPan)
    }

    /// prefetchWindow(for:) asks at once, at full quality, for the window a zoom spring will rest
    /// on, so the engine renders it (a cold photo decodes its full resolution) while the spring
    /// runs. The spring itself changes the zoom, invisibly at the end, for about a second, and no
    /// draft is asked for meanwhile (`lastZoomChange`): without this the first request went at the
    /// settle, 0.8 s later than drafts used to start it. A finger's glide keeps the zoom and its
    /// drafts; a scroll glide (`evenIfPanning`) asks for its resting window alone, as the zoom does.
    private func prefetchWindow(for target: CanvasTransform, evenIfPanning: Bool = false) {
        prefetchTarget = nil

        guard evenIfPanning || target.zoom != canvasTransform.zoom,
              let window = visibleWindow(for: target)
        else { return }

        prefetchTarget = target

        guard window != sentWindow || isSentWindowDraft else { return }

        sentWindow            = window
        isSentWindowDraft     = false
        lastWindowSendTime    = CACurrentMediaTime()
        isWindowUpdatePending = false

        FrameTimingProbe.shared.measure("WindowChange") { onVisibleWindowChange?(window, false) }
    }

    /// advanceSpring moves the transform to where the running spring is at `time`, and ends the
    /// spring once it has settled. A spring just started (start time 0) starts on the last frame.
    private func advanceSpring(to time: CFTimeInterval) {
        guard let spring else { return }

        if springStartTime == 0 {
            springStartTime = CanvasSpring.startTime(
                firstFrame    : time,
                previousFrame : lastTickTime,
                frameInterval : frameInterval
            )
        }

        let elapsed = time - springStartTime
        canvasTransform = spring.transform(at: elapsed)

        if spring.isSettled(at: elapsed) {
            self.spring = nil
        }
    }

    /// visibleWindow(for:) is the window a view at `transform` needs (FrameWindow.visible), nil
    /// while the preview is enough.
    private func visibleWindow(for transform: CanvasTransform) -> FrameWindow? {
        FrameWindow.visible(
            transform   : transform,
            viewSize    : bounds.size,
            imageSize   : imageSize,
            photoSize   : nativeSize,
            screenScale : Double(contentScaleFactor),
            maxPixels   : frames.maxPixels
        )
    }

    /// updateVisibleWindow runs on each display tick: it recomputes the wanted window and hands it to
    /// `onVisibleWindowChange` when it changed. While the view moves (a gesture, a spring or the pan
    /// sweep) a new window waits at least `windowRequestInterval` after the last one and goes as a
    /// draft, and none goes until the zoom has held still that long (`lastZoomChange`); at rest the
    /// window goes at once, at full quality, even if its draft already went. Losing the window (nil)
    /// goes at once. Every tick that wants no window raises `windowFloor`: a window still rendering
    /// when the view left it lands later, and wakes a tick of its own.
    private func updateVisibleWindow(at time: CFTimeInterval) {
        if canvasTransform.zoom != lastTickZoom {
            lastTickZoom   = canvasTransform.zoom
            lastZoomChange = time
        }

        let wanted      = visibleWindow(for: canvasTransform)
        let isZooming   = time - lastZoomChange < Self.windowRequestInterval
        let isScrolling = time - scrollProxyMoved < Self.windowRequestInterval
        let isEasing    = !zoomEasing.isIdle
        let isMoving    = isGestureActive || spring != nil || isEasing || isMeasurementPanning || isScrolling

        wantedWindow = wanted

        // A spring resting on a prefetched window asks for nothing on the way and buries nothing.
        if let prefetchTarget, spring?.target == prefetchTarget {
            isWindowUpdatePending = false
            return
        }

        prefetchTarget = nil

        if wanted == nil {
            windowFloor = Self.newestGeneration(frames.published())
        }

        let isCurrent = wanted == sentWindow && (isMoving || !isSentWindowDraft)

        // A draft sent while the proxy scrolled still owes its full window once the scroll is still.
        guard !isCurrent else {
            isWindowUpdatePending = isScrolling && isSentWindowDraft
            return
        }

        // No draft while the zoom changes (see `lastZoomChange`): the window on screen stays meanwhile.
        if wanted != nil, isMoving, isZooming {
            isWindowUpdatePending = true
            return
        }

        if wanted != nil, isMoving, time - lastWindowSendTime < Self.windowRequestInterval {
            isWindowUpdatePending = true
            return
        }

        sentWindow            = wanted
        isSentWindowDraft     = isMoving && wanted != nil
        lastWindowSendTime    = time
        isWindowUpdatePending = false

        FrameTimingProbe.shared.measure("WindowChange") { onVisibleWindowChange?(wanted, isSentWindowDraft) }
    }

    /// isWindowInFlight says the window last asked for has not landed yet: the sink shows another
    /// placement, or a draft where a full window was asked for (InputRecorder).
    private var isWindowInFlight: Bool {
        guard let sentWindow else { return false }

        let shown = frames.published().window

        return shown?.window != sentWindow || (shown?.isDraft == true && !isSentWindowDraft)
    }

    /// wakeForWindowUpdate runs the display link for at least one tick, so `updateVisibleWindow`
    /// sees a view that stopped moving without a redraw (a gesture ending where it rests).
    private func wakeForWindowUpdate() {
        isWindowUpdatePending = true
        displayLink?.isPaused = false
    }

    /// reportZoom runs on each display tick: it hands `onZoomChange` the percentage when its pill
    /// text differs from the last one, and gives VoiceOver the same text as the canvas's value.
    private func reportZoom() {
        guard let scale = zoomScale else { return }

        let percent = scale.percent(forZoom: canvasTransform.zoom)
        let label   = ZoomScale.label(forPercent: percent)

        guard label != reportedPercent else { return }

        reportedPercent    = label
        accessibilityValue = "\(label)%"
        FrameTimingProbe.shared.measure("ZoomPill") { onZoomChange?(percent) }
    }

    // MARK: - Setup

    private func configureLayer(device: some MTLDevice) {
        isOpaque        = true
        backgroundColor = EditorSurround.uiColor

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
        attachment?.clearColor  = EditorSurround.metalClearColor
    }

    private static func touchType(_ type: UITouch.TouchType) -> NSNumber {
        NSNumber(value: type.rawValue)
    }

    private func configureGestures() {
        let doubleTap = UITapGestureRecognizer(target: self, action: #selector(handleDoubleTap(_:)))
        let singleTap = UITapGestureRecognizer(target: self, action: #selector(handleSingleTap(_:)))

        pinchRecognizer.addTarget(self, action: #selector(handlePinch(_:)))
        panRecognizer.addTarget(self, action: #selector(handlePan(_:)))
        pointerPanRecognizer.addTarget(self, action: #selector(handlePan(_:)))
        scrollRecognizer.addTarget(self, action: #selector(handleScroll(_:)))

        holdRecognizer.addTarget(self, action: #selector(handleHold(_:)))

        doubleTap.numberOfTapsRequired         = 2
        holdRecognizer.minimumPressDuration    = Self.holdDuration
        holdRecognizer.numberOfTouchesRequired = 1
        pinchRecognizer.delegate               = self
        panRecognizer.delegate                 = self
        pointerPanRecognizer.delegate          = self
        scrollRecognizer.delegate              = self
        holdRecognizer.delegate                = self

        panRecognizer.allowedTouchTypes         = [Self.touchType(.direct), Self.touchType(.pencil)]
        pointerPanRecognizer.allowedTouchTypes  = [Self.touchType(.indirectPointer)]
        // No touches at all: the scroll recognizer only hears trackpad and wheel scroll events.
        scrollRecognizer.allowedTouchTypes      = []
        scrollRecognizer.allowedScrollTypesMask = .all

        singleTap.require(toFail: doubleTap)
        singleTap.require(toFail: holdRecognizer)

        addGestureRecognizer(pinchRecognizer)
        addGestureRecognizer(panRecognizer)
        addGestureRecognizer(pointerPanRecognizer)
        addGestureRecognizer(scrollRecognizer)
        configureScrollProxy()
        addGestureRecognizer(holdRecognizer)
        addGestureRecognizer(doubleTap)
        addGestureRecognizer(singleTap)
    }

    /// configureScrollProxy hides the proxy scroll view in the canvas and moves its pan recognizer
    /// onto the canvas, limited to scroll events: touches never reach the proxy (see `scrollProxy`).
    private func configureScrollProxy() {
        let side = Self.scrollProxyContent

        scrollProxy.isUserInteractionEnabled       = false
        scrollProxy.accessibilityElementsHidden    = true
        scrollProxy.showsHorizontalScrollIndicator = false
        scrollProxy.showsVerticalScrollIndicator   = false
        scrollProxy.scrollsToTop                   = false
        scrollProxy.bounces                        = false
        scrollProxy.contentInsetAdjustmentBehavior = .never
        scrollProxy.contentSize                    = CGSize(width: side, height: side)
        scrollProxy.delegate                       = self

        // Its offset sits far from the top, so the toolbar's scroll edge effect showed as scrolled.
        scrollProxy.topEdgeEffect.isHidden    = true
        scrollProxy.bottomEdgeEffect.isHidden = true
        scrollProxy.leftEdgeEffect.isHidden   = true
        scrollProxy.rightEdgeEffect.isHidden  = true

        scrollProxy.panGestureRecognizer.allowedTouchTypes      = []
        scrollProxy.panGestureRecognizer.allowedScrollTypesMask = .all

        addSubview(scrollProxy)
        addGestureRecognizer(scrollProxy.panGestureRecognizer)
        recentreScrollProxy()
    }

    /// recentreScrollProxy puts the proxy's offset back in the middle of its content without
    /// moving the image. Only between gestures: a drag computes its offsets from where it began.
    private func recentreScrollProxy() {
        let middle = CGPoint(x: Self.scrollProxyContent / 2, y: Self.scrollProxyContent / 2)

        scrollProxyOffset         = middle
        scrollProxy.contentOffset = middle
    }

    /// stopScrollGlide ends the proxy's deceleration or wheel animation where it is, for a gesture
    /// that takes over or an image that reached an edge.
    private func stopScrollGlide() {
        guard isScrollProxyGliding else { return }

        holdScrollProxy()
    }

    /// holdScrollProxy stops the proxy where it is, whatever moves it, without moving the image.
    private func holdScrollProxy() {
        // The stop can report an offset hundreds of points past the glide's last step (F5 recording):
        // the image stays where it was seen, so that travel is never applied.
        isScrollProxyAnimating = false
        isStoppingScrollGlide  = true
        scrollProxy.setContentOffset(scrollProxy.contentOffset, animated: false)
        isStoppingScrollGlide  = false
        scrollProxyOffset      = scrollProxy.contentOffset
    }

    /// acceptsScroll(fromProxy:withCommand:) splits a trackpad or wheel scroll between the two
    /// recognizers that hear it: with Cmd held it zooms (scrollRecognizer), without it pans (the
    /// proxy's recognizer). Exactly one of them begins.
    nonisolated static func acceptsScroll(fromProxy isProxy: Bool, withCommand isCommand: Bool) -> Bool {
        isProxy != isCommand
    }

    /// handleHold starts the before look when the hold begins and ends it when the finger lifts, or
    /// when anything takes the touch away. A tap that never became a hold (failed) says nothing, so
    /// it cannot end a before look the keyboard turned on.
    @objc
    private func handleHold(_ recognizer: UILongPressGestureRecognizer) {
        switch recognizer.state {
            case .began:
                isHolding = true
                onHoldChange?(true)

            case .ended, .cancelled, .failed:
                guard isHolding else { return }

                isHolding = false
                onHoldChange?(false)

            default:
                break
        }
    }

    private func configureAccessibility() {
        isAccessibilityElement = true
        accessibilityLabel     = String(localized: "Photo")
        accessibilityTraits    = .image

        configureAccessibilityActions()
    }

    /// configureAccessibilityActions gives VoiceOver the toggle of the before look, named for what it does next.
    private func configureAccessibilityActions() {
        let name = isShowingBefore ? String(localized: "Show Edited") : String(localized: "Show Original")

        accessibilityCustomActions = [
            UIAccessibilityCustomAction(name: name) { [weak self] _ in
                self?.onToggleBefore?()
                return true
            }
        ]
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

extension CanvasView: UIScrollViewDelegate {

    /// scrollViewWillBeginDragging takes the image for a new scroll. A trackpad pinch still in
    /// progress then is stale (a trackpad cannot pinch and scroll at once), so it is cancelled at
    /// once instead of `stalePinchTimeout` after its last event (`watchForStalePinch`).
    func scrollViewWillBeginDragging(_ scrollView: UIScrollView) {
        cancelStaleTrackpadPinch()

        spring = nil

        if !scrollView.isDecelerating {
            recentreScrollProxy()
        }
    }

    /// scrollViewDidScroll moves the image by the proxy's travel since the last call, the other way
    /// (content follows the fingers), rubber-banded past the edges like a finger's pan. A wheel
    /// notch's animation (or a deceleration UIKit still runs) that carries the image past an edge
    /// stops there and springs back, as a scroll view bounces.
    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        let began  = InputRecorder.shared.now
        let offset = scrollView.contentOffset
        let delta  = CGPoint(x: offset.x - scrollProxyOffset.x, y: offset.y - scrollProxyOffset.y)

        scrollProxyOffset = offset

        guard delta != .zero, !isStoppingScrollGlide else { return }

        defer {
            InputRecorder.shared.event(
                "scroll",
                state      : scrollView.panGestureRecognizer.state,
                rawX       : Double(delta.x),
                rawY       : Double(delta.y),
                factor     : 1,
                zoom       : canvasTransform.zoom,
                isIndirect : true,
                began      : began
            )
        }

        spring                 = nil
        scrollProxyMoved       = CACurrentMediaTime()
        isScrollProxyAnimating = !scrollView.isDragging && !scrollView.isDecelerating
        canvasTransform.pan(
            by        : CGPoint(x: -delta.x, y: -delta.y),
            viewSize  : bounds.size,
            imageSize : imageSize
        )

        let resting = canvasTransform.settled(viewSize: bounds.size, imageSize: imageSize)

        if isScrollProxyGliding, resting.offset != canvasTransform.offset {
            stopScrollGlide()
            settle(velocity: .zero)
        }
    }

    /// scrollViewWillEndDragging keeps the proxy where the fingers left it and hands the release
    /// velocity (points per millisecond, the other way for the image) to the canvas's momentum spring.
    /// The proxy's own deceleration moved on every other 60 Hz frame (F6 recording), a visible judder.
    func scrollViewWillEndDragging(
        _ scrollView          : UIScrollView,
        withVelocity velocity : CGPoint,
        targetContentOffset   : UnsafeMutablePointer<CGPoint>
    ) {
        targetContentOffset.pointee = scrollView.contentOffset
        scrollReleaseVelocity       = SIMD2(-Double(velocity.x), -Double(velocity.y)) * 1000
    }

    /// scrollViewDidEndDragging starts the glide with the release velocity, even if UIKit still says
    /// it will decelerate (`scrollViewWillBeginDecelerating` then stops that deceleration). The
    /// recorder logs the release as an `ended` scroll row with the image velocity (points per second).
    func scrollViewDidEndDragging(_ scrollView: UIScrollView, willDecelerate decelerate: Bool) {
        let began    = InputRecorder.shared.now
        let velocity = scrollReleaseVelocity

        scrollReleaseVelocity = .zero

        defer {
            InputRecorder.shared.event(
                "scroll",
                state      : .ended,
                rawX       : velocity.x,
                rawY       : velocity.y,
                factor     : 1,
                zoom       : canvasTransform.zoom,
                isIndirect : true,
                began      : began
            )
        }

        settle(velocity: velocity, isScrollGlide: true)
    }

    /// scrollViewWillBeginDecelerating stops a deceleration UIKit starts although the target offset
    /// was the current one (this iPadOS never does): the canvas's own spring already glides, and the
    /// deceleration's first step would end it and fall back to the 30 Hz judder.
    func scrollViewWillBeginDecelerating(_ scrollView: UIScrollView) {
        holdScrollProxy()
    }

    func scrollViewDidEndDecelerating(_ scrollView: UIScrollView) {
        isScrollProxyAnimating = false
        settle(velocity: .zero)
        recentreScrollProxy()
    }

    /// A wheel's notch scrolls as an animation, not a deceleration: its end settles the same way.
    func scrollViewDidEndScrollingAnimation(_ scrollView: UIScrollView) {
        isScrollProxyAnimating = false
        settle(velocity: .zero)
    }
}

extension CanvasView: UIGestureRecognizerDelegate {

    /// Pinch and pan run together, so two fingers can zoom and move the image in one gesture. The hold
    /// runs alone: once it begins, a pending pan, pinch or double tap fails for that touch, so the
    /// image stays still under a drifting finger; a pan or pinch already moving keeps the hold out.
    func gestureRecognizer(
        _ gestureRecognizer: UIGestureRecognizer,
        shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
    ) -> Bool {
        gestureRecognizer !== holdRecognizer && otherGestureRecognizer !== holdRecognizer
    }
}
