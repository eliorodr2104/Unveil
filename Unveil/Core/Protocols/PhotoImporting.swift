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

    /// Copies a user-picked file into Application Support/Imports and returns the copy.
    func importCopy(of pickedURL: URL) throws(PhotoImportError) -> URL
}
