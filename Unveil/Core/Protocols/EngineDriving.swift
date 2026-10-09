//
//  EngineDriving.swift
//  Unveil
//

import Foundation

/// EngineDriving is the whole surface the editor sees of the engine: open a photo, set a
/// develop control, ask for a preview, and hold the engine still while the app is in the background.
///
/// Callers never touch the FFI: the editor, the lifecycle observer and the memory monitor depend on this
/// protocol, so each can be tested against a fake. The pixels do not come back from these calls: a render
/// lands in `frames` on the engine thread, and `requestPreview` only returns the generation it will carry.
// Nonisolated: the engine thread and background tasks call it, and the app default is MainActor.
nonisolated protocol EngineDriving: AnyObject, Sendable {

    /// The shared buffers the engine fills. Fixed for the life of the engine.
    var frames: FrameSink { get }

    func openPhoto(at fileURL: URL) async throws(EngineError) -> PhotoID
    func set(_ adjustment: DevelopAdjustmentKind, to value: Double) async throws(EngineError)

    /// The current value of each develop control of the active photo, as the engine holds it: the
    /// photo's saved settings, or the as-shot values for a first open.
    func currentValues() async throws(EngineError) -> [DevelopAdjustmentKind: Double]

    /// Starts a render and returns its generation without waiting for it. That generation may never be
    /// delivered if a newer request overtakes it, so wait for a frame at or above it, not equal to it.
    func requestPreview(maxPixels: Int, draft: Bool) throws(EngineError) -> UInt64

    func suspend() throws(EngineError)
    func resume() throws(EngineError)
    func setMemoryBudget(bytes: UInt64)
}
