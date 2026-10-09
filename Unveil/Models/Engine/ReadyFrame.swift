//
//  ReadyFrame.swift
//  Unveil
//

// nonisolated: FrameSink publishes it from the engine thread, and the canvas reads it on the main actor.
/// ReadyFrame describes the newest frame a FrameSink holds; the canvas reads it, never the pixels' owner.
/// The pixels stay in the sink's shared buffer, so a ReadyFrame is a few numbers, cheap to copy across threads.
nonisolated struct ReadyFrame: Equatable, Sendable {

    let bufferIndex : Int      // Which of the two shared MTLBuffers holds the pixels.
    let width       : Int
    let height      : Int
    let bytesPerRow : Int      // Row stride in bytes, aligned for rgba8Unorm textures.
    let generation  : UInt64   // Engine render counter: FrameSink drops anything at or below it.
    let isDraft     : Bool
}
