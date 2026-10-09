//
//  PhotoImporterTests.swift
//  UnveilTests
//

import Foundation
import Testing
@testable import Unveil

struct PhotoImporterTests {

    @Test
    func aPickedFileIsCopiedIntoImports() throws {
        let source = FileManager.default.temporaryDirectory.appending(path: "picked-\(UUID()).ARW")
        try Data([1, 2, 3]).write(to: source)

        let copy = try PhotoImporter().importCopy(of: source)

        #expect(copy.path().contains("/Imports/"))
        #expect(copy.lastPathComponent.hasSuffix("-\(source.lastPathComponent)"))
        #expect(UUID(uuidString: String(copy.lastPathComponent.prefix(36))) != nil)
        #expect(try Data(contentsOf: copy) == Data([1, 2, 3]))
    }

    @Test
    func aMissingFileIsACopyFailure() {
        let missing = URL(filePath: "/nonexistent/\(UUID()).ARW")
        #expect(throws: PhotoImportError.self) {
            _ = try PhotoImporter().importCopy(of: missing)
        }
    }
}
