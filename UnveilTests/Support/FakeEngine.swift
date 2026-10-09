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
        var calls           : [String] = []
        var failNextOpen    : EngineError?
        var failNextValues  : EngineError?
        var openedPhotos    : UInt64   = 0
        var storedValues    : [DevelopAdjustmentKind: Double] = [:]
    }

    private let state = Mutex(State())

    let frames = FrameSink(device: MTLCreateSystemDefaultDevice()!, maxPixels: 64)

    var budgets  : [UInt64] { state.withLock { $0.budgets } }
    var lifecycle: [String] { state.withLock { $0.lifecycle } }

    /// Every openPhoto, select, set and requestPreview in order, as "open <path>", "select <id>",
    /// "set <id> <value>" and "preview <pixels> draft|full".
    var calls: [String] { state.withLock { $0.calls } }

    /// The error the next openPhoto throws, once. It clears itself when thrown.
    var failNextOpen: EngineError? {
        get { state.withLock { $0.failNextOpen } }
        set { state.withLock { $0.failNextOpen = newValue } }
    }

    /// The error the next currentValues throws, once, like a read that times out after a good open.
    var failNextValues: EngineError? {
        get { state.withLock { $0.failNextValues } }
        set { state.withLock { $0.failNextValues = newValue } }
    }

    /// The values currentValues reports on top of the kind defaults, like a photo reopened with saved settings.
    var storedValues: [DevelopAdjustmentKind: Double] {
        get { state.withLock { $0.storedValues } }
        set { state.withLock { $0.storedValues = newValue } }
    }

    func currentValues() async throws(EngineError) -> [DevelopAdjustmentKind: Double] {
        let (stored, failure) = state.withLock {
            let failure = $0.failNextValues
            $0.failNextValues = nil
            return ($0.storedValues, failure)
        }

        if let failure {
            throw failure
        }

        return Dictionary(uniqueKeysWithValues: DevelopAdjustmentKind.allCases.map {
            ($0, stored[$0] ?? $0.defaultValue)
        })
    }

    /// openPhoto hands out ids 1, 2, 3... one per successful open, so a test can tell photos apart.
    func openPhoto(at fileURL: URL) async throws(EngineError) -> PhotoID {
        let (failure, id) = state.withLock {
            $0.calls.append("open \(fileURL.path)")

            let failure = $0.failNextOpen
            $0.failNextOpen = nil

            if failure == nil {
                $0.openedPhotos += 1
            }
            return (failure, $0.openedPhotos)
        }

        if let failure {
            throw failure
        }

        return PhotoID(rawValue: id)
    }

    func select(_ photo: PhotoID) async throws(EngineError) {
        state.withLock { $0.calls.append("select \(photo.rawValue)") }
    }

    /// Yields once after recording, like a real `set` that is still queued when the caller moves on:
    /// a view model that does not chain its calls shows up as a reordered `calls`.
    func set(_ adjustment: DevelopAdjustmentKind, to value: Double) async throws(EngineError) {
        state.withLock { $0.calls.append("set \(adjustment.rawValue) \(value)") }
        await Task.yield()
    }

    func requestPreview(maxPixels: Int, draft: Bool) throws(EngineError) -> UInt64 {
        state.withLock {
            $0.calls.append("preview \(maxPixels) \(draft ? "draft" : "full")")
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
