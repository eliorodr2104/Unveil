//
//  FrameWindow.swift
//  Unveil
//

import CoreGraphics
import Foundation

/// FrameWindow is one window of the photo's frame rendered at a zoom: the frame is the whole photo
/// scaled to `frameWidth x frameHeight` logical pixels, and the window is its `width x height`
/// part at (`x`, `y`), y down. Physical draft samples may be fewer; they never change this coverage.
///
/// The canvas asks for one (`visible`) when it shows the photo above the whole-frame preview's own
/// resolution, so the part on screen is rendered from the photo's real pixels instead of an upscaled
/// preview. A window frame from the engine carries one back, saying where its pixels go: the canvas
/// draws it over the preview at `quad(in:)`.
nonisolated struct FrameWindow: Equatable, Sendable {

    let frameWidth : Int
    let frameHeight: Int
    let x          : Int
    let y          : Int
    let width      : Int
    let height     : Int

    /// margin is how far past the visible part a window reaches, in frame pixels. Spatial stages
    /// (sharpening, noise reduction, clarity) read their neighbours, so the engine's window is
    /// slightly off near its edges (up to ΔE 6 on the test RAWs, none 32 px in), and a short pan
    /// stays inside it.
    static let margin = 32

    /// realPixelsTolerance lets a zoom within 0.2 % of 100 % (a spring that stopped a hair short)
    /// render the photo's real pixels, not a frame resampled by a pixel or two.
    static let realPixelsTolerance = 0.002

    /// frameLong is the frame's long edge, the size the engine is asked for: it fits the frame in
    /// a square of that side, like a preview in `max_pixels`, so the long edge comes back exactly
    /// and the short one follows the frame's own aspect.
    var frameLong: Int { max(frameWidth, frameHeight) }

    /// visible(transform:viewSize:imageSize:photoSize:screenScale:maxPixels:) is the window the canvas
    /// needs, or nil while the preview is enough: at or below one screen pixel per preview pixel, or
    /// while a size is unknown, or with nothing of the photo on screen.
    ///
    /// `photoSize` is the whole frame at the photo's own resolution: the engine's native size of the
    /// oriented, cropped, straightened frame (ReadyFrame), so 100 % of a crop is the crop's pixels.
    /// The frame is the photo at the zoom on screen, capped at those real pixels (100 %): above that
    /// the canvas magnifies the 100 % frame instead of asking for a bigger one. Its long edge is
    /// `photoSize`'s times that scale and its short edge follows `photoSize`'s aspect, which can
    /// round a pixel off the engine's (see `frameLong`). The window is the visible part of the image
    /// plus `margin`, clamped to the frame, and at most `maxPixels` on a side (the sink's buffers),
    /// centred on the visible part when it is cut.
    static func visible(
        transform  : CanvasTransform,
        viewSize   : CGSize,
        imageSize  : CGSize,
        photoSize  : CGSize,
        screenScale: Double,
        maxPixels  : Int
    ) -> FrameWindow? {
        let photoLong   = Double(max(photoSize.width, photoSize.height))
        let previewLong = Double(max(imageSize.width, imageSize.height))

        guard let fitScale = CanvasTransform.fitScale(viewSize: viewSize, imageSize: imageSize),
              screenScale > 0,
              photoLong   > 0,
              maxPixels   > 0
        else {
            return nil
        }

        let screenPerPreview = fitScale * transform.zoom * screenScale
        let photoPerScreen   = screenPerPreview * previewLong / photoLong
        let frameScale       = photoPerScreen >= 1 - Self.realPixelsTolerance ? 1 : photoPerScreen
        let frameLong        = (photoLong * frameScale).rounded()

        // A frame no larger than the preview adds nothing the preview does not already show.
        guard screenPerPreview > 1, frameLong > previewLong else { return nil }

        let photoShort  = Double(min(photoSize.width, photoSize.height))
        let frameShort  = max((frameLong * photoShort / photoLong).rounded(), 1)
        let isLandscape = photoSize.width >= photoSize.height
        let frameSize   = isLandscape ? SIMD2(frameLong, frameShort) : SIMD2(frameShort, frameLong)

        let topLeft     = transform.imagePoint(at: .zero, viewSize: viewSize, imageSize: imageSize)
        let bottomRight = transform.imagePoint(
            at        : CGPoint(x: viewSize.width, y: viewSize.height),
            viewSize  : viewSize,
            imageSize : imageSize
        )
        let preview = SIMD2(Double(imageSize.width), Double(imageSize.height))
        let inFrame = { (point: SIMD2<Double>) in
            (point / preview * frameSize).clamped(lowerBound: .zero, upperBound: frameSize)
        }
        let shownLower = inFrame(SIMD2(Double(topLeft.x), Double(topLeft.y)))
        let shownUpper = inFrame(SIMD2(Double(bottomRight.x), Double(bottomRight.y)))

        guard shownUpper.x > shownLower.x, shownUpper.y > shownLower.y else { return nil }

        let margin = Double(Self.margin)
        let lower  = (shownLower - margin).rounded(.down).clamped(lowerBound: .zero, upperBound: frameSize)
        let upper  = (shownUpper + margin).rounded(.up).clamped(lowerBound: .zero, upperBound: frameSize)
        var origin = lower
        var size   = upper - lower

        // Wider than the buffers (a view larger than the sink): keep the middle of what is shown.
        for axis in 0 ..< 2 where size[axis] > Double(maxPixels) {
            let center   = (shownLower[axis] + shownUpper[axis]) / 2
            size[axis]   = Double(maxPixels)
            origin[axis] = min(max((center - size[axis] / 2).rounded(.down), 0), frameSize[axis] - size[axis])
        }

        return FrameWindow(
            frameWidth  : Int(frameSize.x),
            frameHeight : Int(frameSize.y),
            x           : Int(origin.x),
            y           : Int(origin.y),
            width       : Int(size.x),
            height      : Int(size.y)
        )
    }

    /// hasSameFrame(as:) is true when both windows belong to the same frame: the same long edge,
    /// which the engine renders exactly as asked (the short edge may differ by the estimate's pixel).
    func hasSameFrame(as other: FrameWindow) -> Bool {
        frameLong == other.frameLong
    }

    /// quad(in:) is where the window goes, in NDC (y up), given the NDC corners of the whole image:
    /// the window covers the fraction `x / frameWidth ... (x + width) / frameWidth` of the image
    /// across, and the same fraction of its height down from the top.
    func quad(in image: (min: SIMD2<Float>, max: SIMD2<Float>)) -> (min: SIMD2<Float>, max: SIMD2<Float>) {
        let frame = SIMD2<Float>(Float(frameWidth), Float(frameHeight))
        let start = SIMD2<Float>(Float(x), Float(y)) / frame
        let end   = SIMD2<Float>(Float(x + width), Float(y + height)) / frame
        let span  = image.max - image.min

        return (
            min: SIMD2(image.min.x + start.x * span.x, image.max.y - end.y * span.y),
            max: SIMD2(image.min.x + end.x * span.x, image.max.y - start.y * span.y)
        )
    }
}
