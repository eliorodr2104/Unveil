//
//  PhotoImportError.swift
//  Unveil
//

import Foundation

/// PhotoImportError says why a picked file could not be copied into the app.
nonisolated enum PhotoImportError: Error, Equatable {

    /// The system refused to read the picked file (a permission error, not a missing file).
    case accessDenied(URL)

    /// Any other failure, with the underlying error's description.
    case copyFailed(String)
}
