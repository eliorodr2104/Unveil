//
//  WaitUntil.swift
//  UnveilTests
//

import Foundation

/// WaitTimedOut is what waitUntil throws when its condition never held.
private struct WaitTimedOut: Error, CustomStringConvertible {

    let seconds: Double

    var description: String { "the condition did not hold within \(seconds) s" }
}

/// waitUntil polls `condition` every 20 ms until it holds, for frames and other results that arrive
/// on another thread with no completion to await. It throws once `seconds` have passed.
func waitUntil(seconds: Double, _ condition: () -> Bool) async throws {
    let deadline = ContinuousClock.now + .seconds(seconds)

    while !condition() {
        guard ContinuousClock.now < deadline else { throw WaitTimedOut(seconds: seconds) }
        try await Task.sleep(for: .milliseconds(20))
    }
}
