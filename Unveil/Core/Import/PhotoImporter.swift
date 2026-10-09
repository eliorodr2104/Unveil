//
//  PhotoImporter.swift
//  Unveil
//

import Foundation

/// PhotoImporter copies a picked file into `Application Support/Imports/<UUID>-<name>`.
///
/// The UUID prefix keeps two picks of the same file name from colliding. Copies
/// are never deleted in v0, because the engine's library refers to them by path.
/// The picked URL is opened with `startAccessingSecurityScopedResource()`, which
/// returns false for ordinary URLs (a temp file, say): that is not an error, we
/// copy anyway and only balance the call with `stop...` when it returned true.
// Nonisolated: the project defaults to MainActor, but the copy must run off main.
nonisolated struct PhotoImporter: PhotoImporting {

    func importCopy(of pickedURL: URL) throws(PhotoImportError) -> URL {
        let isScoped = pickedURL.startAccessingSecurityScopedResource()
        defer {
            if isScoped {
                pickedURL.stopAccessingSecurityScopedResource()
            }
        }

        do {
            let imports = URL.applicationSupportDirectory.appending(path: "Imports", directoryHint: .isDirectory)
            try FileManager.default.createDirectory(at: imports, withIntermediateDirectories: true)

            let copy = imports.appending(path: "\(UUID().uuidString)-\(pickedURL.lastPathComponent)")
            try FileManager.default.copyItem(at: pickedURL, to: copy)

            return copy
        } catch let error as CocoaError where error.code == .fileReadNoPermission {
            throw .accessDenied(pickedURL)
        } catch {
            throw .copyFailed(error.localizedDescription)
        }
    }
}
