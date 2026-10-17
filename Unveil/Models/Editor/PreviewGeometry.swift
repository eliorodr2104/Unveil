//
//  PreviewGeometry.swift
//  Unveil
//

import CoreGraphics

/// PreviewGeometry keeps the last full preview as the zoom anchor during a draft sequence.
/// A first draft already has a logical display extent; its smaller texture must not change fit
/// or window selection. A photo change clears the anchor before that photo's first frame arrives.
nonisolated struct PreviewGeometry {

    private(set) var fullSize: CGSize = .zero

    /// observe updates the anchor only for a full preview. Window coverage cannot replace it,
    /// and a crop's draft can show its new framing before the final full frame resets the anchor.
    mutating func observe(_ preview: ReadyFrame) {
        guard !preview.isDraft, preview.window == nil else { return }

        fullSize = preview.displaySize
    }

    /// imageSize uses the full anchor when available, otherwise the first preview's logical size.
    func imageSize(for preview: ReadyFrame?) -> CGSize {
        fullSize != .zero ? fullSize : preview?.displaySize ?? .zero
    }

    /// reset discards the preceding photo's anchor.
    mutating func reset() {
        fullSize = .zero
    }
}
