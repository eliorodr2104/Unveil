//
//  ReadyFrame.swift
//  Unveil
//

import CoreGraphics

// nonisolated: FrameSink publishes it from the engine thread, and the canvas reads it on the main actor.
/// ReadyFrame describes a frame a FrameSink holds; the canvas reads it, never the pixels' owner.
/// The pixels stay in the sink's shared buffer, so a ReadyFrame is a few numbers, cheap to copy across threads.
/// width/height describe the physical texture; displayWidth/displayHeight describe its logical extent.
/// A whole-frame preview has no `window`; a zoom window gives its logical placement in the frame.
/// `nativeWidth x nativeHeight` is the whole frame at the photo's own resolution (cropped and
/// straightened), what 100 % means; 0 when the engine did not say.
nonisolated struct ReadyFrame: Equatable, Sendable {

    let bufferIndex  : Int            // Which shared MTLBuffer holds the physical pixels.
    let width        : Int
    let height       : Int
    let displayWidth : Int
    let displayHeight: Int
    let bytesPerRow  : Int            // Physical row stride, aligned for rgba8Unorm textures.
    let generation   : UInt64         // Engine render counter: older generations are dropped.
    let isDraft      : Bool
    let window       : FrameWindow?   // Logical placement; nil for a whole-frame preview.
    let nativeWidth  : Int
    let nativeHeight : Int

    /// displaySize is the logical coverage, independent of the number of texture samples.
    var displaySize: CGSize { CGSize(width: displayWidth, height: displayHeight) }

    /// hasIdentityRaster permits native sampling and pixel alignment when one texel covers one
    /// logical pixel. Reduced drafts keep bilinear filtering even when the canvas is at 100 %.
    var hasIdentityRaster: Bool { width == displayWidth && height == displayHeight }
}
