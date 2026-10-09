//
//  PhotoImporting.swift
//  Unveil
//

import Foundation

/// PhotoImporting turns a file the user picked into a copy the app owns.
///
/// The picked URL may be security-scoped and may vanish at any time, while the
/// engine needs a stable path for as long as the photo stays in the library.
/// Conformers must be usable off the main actor: a RAW copy can take seconds.
// Nonisolated: the requirement must be callable off main, like its conformers.
nonisolated protocol PhotoImporting: Sendable {

    /// Copies a user-picked file into Application Support/Imports and returns the copy. `isNew` is
    /// false when an identical copy was already there and is returned as is: the caller may delete
    /// a new copy it no longer needs, never a reused one, which the library may still point at.
    func importCopy(of pickedURL: URL) throws(PhotoImportError) -> (url: URL, isNew: Bool)
}
