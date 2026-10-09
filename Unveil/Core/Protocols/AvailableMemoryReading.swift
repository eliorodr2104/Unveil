//
//  AvailableMemoryReading.swift
//  Unveil
//

/// AvailableMemoryReading says how many bytes this process may still allocate.
///
/// A reading of 0 means the system gave no usable number: the process is not an app
/// (the simulator), or it is already over its memory limit.
// Nonisolated: the warning handler is a @Sendable closure, outside MainActor isolation.
nonisolated protocol AvailableMemoryReading: Sendable {

    func availableBytes() -> UInt64
}
