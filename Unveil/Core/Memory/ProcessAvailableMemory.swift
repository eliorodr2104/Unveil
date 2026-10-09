//
//  ProcessAvailableMemory.swift
//  Unveil
//

import os

/// ProcessAvailableMemory reads the memory the system still lets this app use.
///
/// `os_proc_available_memory()` returns 0 when the process is not an app (the simulator)
/// or when it already exceeds its memory limit; the two cases cannot be told apart.
// Nonisolated: it is read from a @Sendable notification handler, outside MainActor isolation.
nonisolated struct ProcessAvailableMemory: AvailableMemoryReading {

    func availableBytes() -> UInt64 {
        UInt64(os_proc_available_memory())
    }
}
