//
//  FakeImporter.swift
//  UnveilTests
//

import Foundation
@testable import Unveil

/// FakeImporter stands in for the file copy: it hands back the URL it receives, touching no disk.
nonisolated struct FakeImporter: PhotoImporting {

    func importCopy(of pickedURL: URL) throws(PhotoImportError) -> URL {
        pickedURL
    }
}
