//
//  FakeEngine.swift
//  UnveilTests
//

import Foundation
import Metal
import Synchronization
@testable import Unveil

/// FakeEngine stands in for the engine in tests: it records what it is told and renders nothing.
///
/// All recorded state lives in one `Mutex<State>`; the test-facing properties are computed over
/// it, because a settable stored property is illegal in a Sendable class.
final class FakeEngine: EngineDriving {

    private struct State {
        var budgets         : [UInt64] = []
        var lifecycle       : [String] = []
        var previewCounter  : UInt64   = 0
    }

    private let state = Mutex(State())

    let frames = FrameSink(device: MTLCreateSystemDefaultDevice()!, maxPixels: 64)

    var budgets  : [UInt64] { state.withLock { $0.budgets } }
    var lifecycle: [String] { state.withLock { $0.lifecycle } }

    func openPhoto(at fileURL: URL) async throws(EngineError) -> PhotoID {
        PhotoID(rawValue: 1)
    }

    func set(_ adjustment: DevelopAdjustmentKind, to value: Double) async throws(EngineError) {}

    func requestPreview(maxPixels: Int, draft: Bool) throws(EngineError) -> UInt64 {
        state.withLock {
            $0.previewCounter += 1
            return $0.previewCounter
        }
    }

    func suspend() throws(EngineError) {
        state.withLock { $0.lifecycle.append("suspend") }
    }

    func resume() throws(EngineError) {
        state.withLock { $0.lifecycle.append("resume") }
    }

    func setMemoryBudget(bytes: UInt64) {
        state.withLock { $0.budgets.append(bytes) }
    }
}
