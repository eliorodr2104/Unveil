//
//  PhotoID.swift
//  Unveil
//

// nonisolated: the engine thread hands these to the main actor, and the app default is MainActor.
/// PhotoID names one photo in the engine's library. The engine assigns it on import, and the app passes it back as is.
nonisolated struct PhotoID: Hashable, Sendable {

    let rawValue: UInt64
}
