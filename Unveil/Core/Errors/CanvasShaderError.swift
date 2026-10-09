//
//  CanvasShaderError.swift
//  Unveil
//

/// CanvasShaderError says why the canvas pipeline could not be built, past what Metal's own
/// compiler error already reports.
nonisolated enum CanvasShaderError: Error, Equatable {

    /// The compiled library has no function with this name: the source and its names drifted apart.
    case missingFunction(String)
}
