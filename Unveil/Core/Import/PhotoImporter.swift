//
//  PhotoImporter.swift
//  Unveil
//

import CryptoKit
import Foundation

/// PhotoImporter copies a picked file into `Application Support/Imports/<sha256>-<name>`.
///
/// The name is a function of the bytes, so reopening the same file finds its copy already there
/// and reuses it instead of copying again: the folder holds one copy per distinct file, not one
/// per open. The hash, not the name with size and mtime, keys the copy because two different
/// photos can share all three (same camera numbering, a tool that restores mtime), and reusing
/// the wrong copy would silently open the wrong photo. Hashing costs one read of the file, less
/// than the copy it saves on a reopen; the name is kept after the hash only for a human reader.
///
/// The copy lands under its final name by a move from the temporary folder, so a copy cut short
/// (the app killed mid-copy) never sits there looking complete. The folder is excluded from the
/// iCloud backup: the copies are the user's own files, which they still have.
///
/// The picked URL is opened with `startAccessingSecurityScopedResource()`, which returns false for
/// ordinary URLs (a temp file, say): that is not an error, we copy anyway and only balance the
/// call with `stop...` when it returned true.
// Nonisolated: the project defaults to MainActor, but the copy must run off main.
nonisolated struct PhotoImporter: PhotoImporting {

    let importsDirectory: URL

    init(
        importsDirectory: URL = URL.applicationSupportDirectory.appending(path: "Imports", directoryHint: .isDirectory)
    ) {
        self.importsDirectory = importsDirectory
    }

    func importCopy(of pickedURL: URL) throws(PhotoImportError) -> (url: URL, isNew: Bool) {
        let isScoped = pickedURL.startAccessingSecurityScopedResource()
        defer {
            if isScoped {
                pickedURL.stopAccessingSecurityScopedResource()
            }
        }

        do {
            let fileManager = FileManager.default
            var imports     = importsDirectory

            try fileManager.createDirectory(at: imports, withIntermediateDirectories: true)

            var noBackup = URLResourceValues()
            noBackup.isExcludedFromBackup = true
            try imports.setResourceValues(noBackup)

            let hash = try Self.contentHash(of: pickedURL)
            let copy = imports.appending(path: "\(hash)-\(pickedURL.lastPathComponent)")

            if fileManager.fileExists(atPath: copy.path(percentEncoded: false)) {
                return (copy, false)
            }

            let partial = URL.temporaryDirectory.appending(path: "import-\(UUID().uuidString)")
            try fileManager.copyItem(at: pickedURL, to: partial)
            try fileManager.moveItem(at: partial, to: copy)

            return (copy, true)
        } catch let error as CocoaError where error.code == .fileReadNoPermission {
            throw .accessDenied(pickedURL)
        } catch {
            throw .copyFailed(error.localizedDescription)
        }
    }

    /// contentHash is the file's SHA-256 in hex, read in 1 MB chunks so a 100 MB RAW never sits
    /// whole in memory next to the engine's working set.
    private static func contentHash(of url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }

        var hasher = SHA256()
        while let chunk = try handle.read(upToCount: 1 << 20), !chunk.isEmpty {
            hasher.update(data: chunk)
        }

        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}
