//
//  FixedMemory.swift
//  UnveilTests
//

@testable import Unveil

/// FixedMemory reports the same available memory every time, for tests.
struct FixedMemory: AvailableMemoryReading {

    let bytes: UInt64

    func availableBytes() -> UInt64 {
        bytes
    }
}
