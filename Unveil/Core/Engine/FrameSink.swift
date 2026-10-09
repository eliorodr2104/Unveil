//
//  FrameSink.swift
//  Unveil
//

import Metal
import os
import Synchronization

/// FrameSink holds two shared MTLBuffers that alternate (double buffering): the engine thread
/// copies a finished frame into the one not on display, then publishes it by swapping.
///
/// `receive(...)` runs on the engine thread, `latest()` and `buffers` are read by the canvas, and
/// the only state they share (which buffer is next, the newest ReadyFrame, the wake-up callback)
/// sits behind one Mutex. The pixel copy runs outside it, so the canvas never waits on a memcpy,
/// and `onFrame` is called after the unlock for the same reason: it may re-enter `latest()`.
///
/// `maxPixels` is the long edge, as in the engine's preview request: a frame is accepted when both
/// its width and its height fit, and every buffer is sized once for the worst case, a square
/// `maxPixels` frame with aligned rows. Nothing is allocated after init.
///
/// Rows are padded to `minimumLinearTextureAlignment(for: .rgba8Unorm)`, so the canvas can wrap a
/// buffer in a texture with no copy. The engine's own stride is `width * 4` and may not be aligned,
/// which is why the copy goes row by row.
///
/// Sendable is asserted, not inferred: MTLBuffer is not Sendable, but `buffers` are immutable
/// references and the only shared mutable state is inside the Mutex. The pixel bytes are not
/// guarded: one producer writes the unpublished buffer, and readers sample the published one.
/// They can overlap once, when two publishes land inside one in-flight GPU pass that still
/// samples the buffer being rewritten. That stays in bounds, is visual only (a torn frame) and
/// corrects itself on the next frame.
// Nonisolated: the engine thread calls it, and the app default is MainActor.
nonisolated final class FrameSink: @unchecked Sendable {

    let buffers: (any MTLBuffer, any MTLBuffer)

    private let maxPixels   : Int
    private let rowAlignment: Int
    private let state       : Mutex<State>
    private let logger      = Logger(subsystem: "com.eliorodr2104.unveil", category: "FrameSink")

    private struct State {

        var nextBufferIndex: Int                     = 0
        var latestFrame    : ReadyFrame?             = nil
        var onFrame        : (@Sendable () -> Void)? = nil
    }

    /// onFrame wakes the canvas after each published frame. Set it once at startup.
    var onFrame: (@Sendable () -> Void)? {
        get { state.withLock { $0.onFrame } }
        set { state.withLock { $0.onFrame = newValue } }
    }

    /// init(device:maxPixels:) allocates both buffers for the largest frame, a square of `maxPixels`
    /// (the long edge). A failed allocation here is out of memory at launch: there is nothing to degrade to.
    init(device: some MTLDevice, maxPixels: Int) {
        precondition(maxPixels > 0, "FrameSink: maxPixels must be positive")

        let alignment = device.minimumLinearTextureAlignment(for: .rgba8Unorm)
        let length    = Self.alignUp(maxPixels * 4, to: alignment) * maxPixels

        guard let first  = device.makeBuffer(length: length, options: .storageModeShared),
              let second = device.makeBuffer(length: length, options: .storageModeShared)
        else {
            fatalError("FrameSink: cannot allocate two shared buffers of \(length) bytes")
        }

        self.buffers      = (first, second)
        self.maxPixels    = maxPixels
        self.rowAlignment = alignment
        self.state        = Mutex(State())
    }

    /// latest returns the newest published frame, or nil before the first one.
    func latest() -> ReadyFrame? {
        state.withLock { $0.latestFrame }
    }

    /// receive copies one engine frame into the buffer not currently published, then publishes it.
    /// Called on the engine thread. A frame at or below the last published generation, or larger than
    /// `maxPixels` on either edge, is dropped: the first because a newer one already won, the second
    /// because it would overrun the buffer (it is logged, since it means a request outgrew the sink).
    ///
    /// `rgba` must point to `height` rows of `stride` bytes, each holding at least `width * 4`.
    /// The pointer is only read during the call.
    func receive(
        rgba      : UnsafeRawPointer,
        width     : Int,
        height    : Int,
        stride    : Int,
        generation: UInt64,
        isDraft   : Bool
    ) {
        guard width > 0, height > 0, stride >= width * 4 else {
            logger.error("Dropped a malformed frame: \(width)x\(height), stride \(stride)")
            return
        }

        guard width <= maxPixels, height <= maxPixels else {
            logger.error("Dropped an oversized frame: \(width)x\(height), long edge limit \(self.maxPixels)")
            return
        }

        // ponytail: two publishes inside one in-flight GPU pass can tear the buffer it samples;
        // upgrade is a third buffer plus an in-use pin, if T13 measures it.
        let target = state.withLock { current -> Int? in
            guard Self.isNewer(generation, than: current.latestFrame) else { return nil }
            return current.nextBufferIndex
        }

        guard let target else { return }

        let bytesPerRow = Self.alignUp(width * 4, to: rowAlignment)

        copyRows(
            from        : rgba,
            to          : target,
            width       : width,
            height      : height,
            stride      : stride,
            bytesPerRow : bytesPerRow
        )

        let frame = ReadyFrame(
            bufferIndex : target,
            width       : width,
            height      : height,
            bytesPerRow : bytesPerRow,
            generation  : generation,
            isDraft     : isDraft
        )

        let wake = state.withLock { current -> (@Sendable () -> Void)? in
            guard Self.isNewer(generation, than: current.latestFrame) else { return nil }

            current.latestFrame     = frame
            current.nextBufferIndex = 1 - target
            return current.onFrame
        }

        wake?()
    }

    /// copyRows copies `height` rows of `width * 4` bytes, one memcpy each, into the buffer at `index`.
    ///
    /// The destination is the buffer's CPU mapping, owned by the buffer and alive as long as `self`.
    /// Row `r` ends at `r * bytesPerRow + width * 4`, which never exceeds the length set in `init`,
    /// because the caller checked `width` and `height` against `maxPixels` and `bytesPerRow` grows
    /// monotonically with `width`. The padding at the end of each row is left untouched.
    private func copyRows(
        from source : UnsafeRawPointer,
        to index    : Int,
        width       : Int,
        height      : Int,
        stride      : Int,
        bytesPerRow : Int
    ) {
        let destination = (index == 0 ? buffers.0 : buffers.1).contents()
        let rowBytes    = width * 4

        for row in 0 ..< height {
            (destination + row * bytesPerRow).copyMemory(from: source + row * stride, byteCount: rowBytes)
        }
    }

    private static func isNewer(_ generation: UInt64, than frame: ReadyFrame?) -> Bool {
        guard let frame else { return true }
        return generation > frame.generation
    }

    private static func alignUp(_ value: Int, to alignment: Int) -> Int {
        (value + alignment - 1) / alignment * alignment
    }
}
