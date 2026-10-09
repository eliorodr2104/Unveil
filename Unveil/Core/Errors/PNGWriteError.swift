//
//  PNGWriteError.swift
//  Unveil
//

// Nonisolated: PNGWriter runs off the main actor, and the app default is MainActor.
/// PNGWriteError is why PNGWriter could not produce a file.
nonisolated enum PNGWriteError: Error, Equatable {

    case badGeometry(String)
    case cannotEncode(String)
}
