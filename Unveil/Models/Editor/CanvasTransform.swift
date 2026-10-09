//
//  CanvasTransform.swift
//  Unveil
//

import CoreGraphics

/// CanvasTransform maps the image into the view: aspect fit at zoom 1, then zoom and pan.
///
/// It is a plain value, a pure function of the gesture state, so it carries only
/// the two numbers the Metal layer needs to place the image quad. Its layout is
/// two plain fields, so the redraw path reads it on every frame without cost.
/// It is not `@frozen`: the compiler rejects that attribute on an internal type.
///
/// The geometry, with a view of `w x h` points and an image of `iw x ih` pixels:
/// the fit scale is `s = min(w / iw, h / ih)`, so at zoom 1 the image is `iw*s`
/// by `ih*s` points and centered in the view. Zoom multiplies that size around
/// the view center, and `offset` then moves the center, in view points with y
/// DOWN (the UIKit convention). The quad is returned in NDC with y UP (the Metal
/// convention), so the vertical offset changes sign on the way out.
nonisolated struct CanvasTransform: Equatable, Sendable {

    // nonisolated: pure value math, callable from the engine thread and from tests,
    // while the project default would pin it to the main actor.

    static let zoomRange: ClosedRange<Double> = 1...16

    /// 1 means aspect fit, clamped to `zoomRange` after every pinch.
    var zoom  : Double = 1

    /// Displacement of the image center from the view center, in view points, y down.
    var offset: SIMD2<Double> = .zero

    /// quad(viewSize:imageSize:) returns the NDC corners of the image rectangle.
    ///
    /// Half the displayed size in points is `iw*s*zoom / 2`; dividing by `w / 2`
    /// (half the view, which is 1 in NDC) gives the NDC half-extent. A degenerate
    /// view or image yields an empty quad instead of dividing by zero.
    func quad(viewSize: CGSize, imageSize: CGSize) -> (min: SIMD2<Float>, max: SIMD2<Float>) {
        guard let fitScale = fitScale(viewSize: viewSize, imageSize: imageSize) else {
            return (.zero, .zero)
        }

        let halfViewWidth   = Double(viewSize.width)   / 2
        let halfViewHeight  = Double(viewSize.height)  / 2
        let halfImageWidth  = Double(imageSize.width)  * fitScale * zoom / 2
        let halfImageHeight = Double(imageSize.height) * fitScale * zoom / 2

        // Offset y is down in view points, NDC y is up: hence the minus on offset.y.
        let minimum = SIMD2<Double>(
            (-halfImageWidth  + offset.x) / halfViewWidth,
            (-halfImageHeight - offset.y) / halfViewHeight
        )
        let maximum = SIMD2<Double>(
            ( halfImageWidth  + offset.x) / halfViewWidth,
            ( halfImageHeight - offset.y) / halfViewHeight
        )

        return (SIMD2<Float>(minimum), SIMD2<Float>(maximum))
    }

    /// imagePoint(at:viewSize:imageSize:) is the inverse of `quad`: it returns the
    /// image pixel (origin top-left, y down) shown under a view point (y down).
    ///
    /// A view point sits `viewPoint - center - offset` points from the image
    /// center, and one image pixel spans `fitScale * zoom` points.
    func imagePoint(at viewPoint: CGPoint, viewSize: CGSize, imageSize: CGSize) -> CGPoint {
        guard let fitScale = fitScale(viewSize: viewSize, imageSize: imageSize) else { return .zero }

        let pointsPerPixel = fitScale * zoom
        let fromCenterX    = Double(viewPoint.x) - Double(viewSize.width)  / 2 - offset.x
        let fromCenterY    = Double(viewPoint.y) - Double(viewSize.height) / 2 - offset.y

        return CGPoint(
            x: fromCenterX / pointsPerPixel + Double(imageSize.width)  / 2,
            y: fromCenterY / pointsPerPixel + Double(imageSize.height) / 2
        )
    }

    /// pinch(by:around:viewSize:) multiplies the zoom by `factor`, clamped, keeping
    /// the image point under `point` (view points, y down) where it is.
    ///
    /// With `d` the point relative to the view center, the image point under it is
    /// `(d - offset) / (fitScale * zoom)`. Asking for the same point after the zoom
    /// changes by `ratio` gives `offset' = d - ratio * (d - offset)`. The fit scale
    /// cancels out, which is why no image size is needed. The ratio comes from the
    /// CLAMPED zoom, so hitting a limit never drags the image. A non-finite or
    /// non-positive factor is ignored.
    mutating func pinch(by factor: Double, around point: CGPoint, viewSize: CGSize) {
        guard factor.isFinite, factor > 0 else { return }

        let clampedZoom = min(max(zoom * factor, Self.zoomRange.lowerBound), Self.zoomRange.upperBound)
        let ratio       = clampedZoom / zoom
        let fromCenter  = SIMD2<Double>(
            Double(point.x) - Double(viewSize.width)  / 2,
            Double(point.y) - Double(viewSize.height) / 2
        )

        offset = fromCenter - ratio * (fromCenter - offset)
        zoom   = clampedZoom
    }

    /// pan(by:) moves the image by a gesture translation, in view points.
    mutating func pan(by translation: CGPoint) {
        offset += SIMD2<Double>(Double(translation.x), Double(translation.y))
    }

    /// reset() returns to aspect fit, centered.
    mutating func reset() {
        self = CanvasTransform()
    }

    /// fitScale(viewSize:imageSize:) is view points per image pixel at zoom 1, or
    /// nil when either size is degenerate.
    private func fitScale(viewSize: CGSize, imageSize: CGSize) -> Double? {
        guard viewSize.width   > 0,
              viewSize.height  > 0,
              imageSize.width  > 0,
              imageSize.height > 0
        else {
            return nil
        }

        return min(
            Double(viewSize.width)  / Double(imageSize.width),
            Double(viewSize.height) / Double(imageSize.height)
        )
    }
}
