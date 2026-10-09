//
//  MemoryBudgetMonitor.swift
//  Unveil
//

import UIKit

/// MemoryBudgetMonitor tells the engine how much memory it may use, and shrinks that on memory warnings.
///
/// At `start()` the budget is a third of what is still available. On each warning it is
/// recomputed from half of the memory available right now, so the 256 MiB floor still holds.
/// A reading of 0 (simulator, or already over the limit) applies nothing: the engine keeps its budget.
// Nonisolated: the observer closure is @Sendable, so it captures only engine and memory.
nonisolated final class MemoryBudgetMonitor<Engine: EngineDriving, Memory: AvailableMemoryReading> {

    private static var floorBytes: UInt64 { 256 << 20 }

    private let engine      : Engine
    private let memory      : Memory

    // The notification token type is an existential declared by Foundation.
    private var warningToken: (any NSObjectProtocol)?

    init(engine: Engine, memory: Memory) {
        self.engine = engine
        self.memory = memory
    }

    deinit {
        if let warningToken {
            NotificationCenter.default.removeObserver(warningToken)
        }
    }

    /// One third of the memory still available, at least 256 MiB.
    static func budget(forAvailable bytes: UInt64) -> UInt64 {
        max(bytes / 3, floorBytes)
    }

    /// Applies the budget now and listens for memory warnings.
    func start() {
        Self.apply(to: engine, memory: memory, halved: false)

        // queue nil: runs on the posting thread (main), outside MainActor isolation; it only
        // calls the engine's thread-safe setMemoryBudget, so it needs no self.
        let engine = engine
        let memory = memory
        warningToken = NotificationCenter.default.addObserver(
            forName : UIApplication.didReceiveMemoryWarningNotification,
            object  : nil,
            queue   : nil
        ) { _ in
            Self.apply(to: engine, memory: memory, halved: true)
        }
    }

    func handleMemoryWarning() {
        Self.apply(to: engine, memory: memory, halved: true)
    }

    private static func apply(to engine: Engine, memory: Memory, halved: Bool) {
        let available = memory.availableBytes()
        guard available > 0 else { return }

        // 0 gives no usable limit, so the engine keeps its budget.
        engine.setMemoryBudget(bytes: budget(forAvailable: halved ? available / 2 : available))
    }
}
