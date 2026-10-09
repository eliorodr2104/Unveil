//
//  FakeImporter.swift
//  UnveilTests
//

import Foundation
@testable import Unveil

/// FakeImporter stands in for the file copy: it hands back the URL it receives as a reused copy, so nothing
/// ever deletes it, touching no disk.
nonisolated struct FakeImporter: PhotoImporting {

    func importCopy(of pickedURL: URL) throws(PhotoImportError) -> (url: URL, isNew: Bool) {
        (pickedURL, false)
    }
}
