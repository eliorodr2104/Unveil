//
//  EngineManagerTests.swift
//  UnveilTests
//

import Foundation
import Synchronization
import Testing
import UIKit
@testable import Unveil

/// Serialized: the engine's GPU switch, memory budget and environment are process-wide,
/// and each manager gets its own library folder because a library is file-locked.
@Suite(.serialized)
struct EngineManagerTests {

    /// A PNG the engine can import: the simulator has no RAW samples. Two colors make two photos;
    /// one color twice is the same bytes, which the engine and the importer treat as one file.
    func fixtureURL(
        color : UIColor = .systemTeal,
        in    folder: URL = FileManager.default.temporaryDirectory
    ) throws -> URL {
        let url      = folder.appending(path: "fixture-\(UUID()).png")
        let format   = UIGraphicsImageRendererFormat()
        format.scale = 1
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: 96, height: 64), format: format)
        let data     = renderer.pngData { context in
            color.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 96, height: 64))
        }

        try data.write(to: url)
        return url
    }

    func makeManager(maxPixels: Int) throws -> EngineManager {
        try EngineManager(dataDirectory: TemporaryEngineLibrary.make(), maxPixels: maxPixels)
    }

    @Test
    func openSetAndPreviewDeliversAFrame() async throws {
        let manager = try makeManager(maxPixels: 512)
        let fixture = try fixtureURL()
        defer { try? FileManager.default.removeItem(at: fixture) }
        _ = try await manager.openPhoto(at: fixture)
        try await manager.set(.exposure, to: 0.5)

        let generation = try manager.requestPreview(
            maxPixels : 512,
            draft     : false,
            before    : false
        )

        // A newer generation may overtake this one, so any frame at or above it counts.
        try await waitUntil(seconds: 30) { (manager.frames.latest()?.generation ?? 0) >= generation }
    }

    /// A window of the fixture at 400 % comes back through the C callback as a window frame, with
    /// its placement, in the sink's window slot: the preview stays where it was. Both carry the
    /// fixture's own 96 x 64 as the native size of the frame.
    @Test
    func aWindowRequestDeliversAPlacedWindowBesideThePreview() async throws {
        let manager = try makeManager(maxPixels: 512)
        let fixture = try fixtureURL()
        defer { try? FileManager.default.removeItem(at: fixture) }
        _ = try await manager.openPhoto(at: fixture)

        let preview = try manager.requestPreview(
            maxPixels : 96,
            draft     : false,
            before    : false
        )
        try await waitUntil(seconds: 30) { (manager.frames.latest()?.generation ?? 0) >= preview }

        let asked = FrameWindow(
            frameWidth  : 384,
            frameHeight : 256,
            x           : 100,
            y           : 40,
            width       : 120,
            height      : 80
        )
        let generation = try manager.requestWindow(
            asked,
            draft     : false,
            before    : false,
            histogram : false
        )

        try await waitUntil(seconds: 30) { (manager.frames.published().window?.generation ?? 0) >= generation }

        let published = manager.frames.published()
        #expect(published.window?.window == asked)
        #expect(published.window?.width == 120 && published.window?.height == 80)
        #expect(published.frame?.generation == preview && published.frame?.window == nil)
        #expect(published.frame?.nativeWidth == 96 && published.frame?.nativeHeight == 64)
        #expect(published.window?.nativeWidth == 96 && published.window?.nativeHeight == 64)
    }

    /// A window that asks for the histogram brings the whole photo's (every pixel of the 512 px
    /// whole frame, not the window's 120 x 80) to the sink's listener under its own generation; a
    /// window that does not ask brings none.
    @Test
    func aWindowThatAsksCarriesTheWholePhotosHistogramToTheSink() async throws {
        let manager = try makeManager(maxPixels: 512)
        let fixture = try fixtureURL()
        defer { try? FileManager.default.removeItem(at: fixture) }
        _ = try await manager.openPhoto(at: fixture)
        let heard = Mutex<[FrameHistogram]>([])
        manager.frames.onHistogram = { histogram in heard.withLock { $0.append(histogram) } }

        let preview = try manager.requestPreview(
            maxPixels : 96,
            draft     : false,
            before    : false
        )
        try await waitUntil(seconds: 30) { (manager.frames.latest()?.generation ?? 0) >= preview }

        let asked = FrameWindow(
            frameWidth  : 384,
            frameHeight : 256,
            x           : 100,
            y           : 40,
            width       : 120,
            height      : 80
        )
        let plain = try manager.requestWindow(
            asked,
            draft     : true,
            before    : false,
            histogram : false
        )
        let carrying = try manager.requestWindow(
            asked,
            draft     : true,
            before    : false,
            histogram : true
        )
        try await waitUntil(seconds: 30) { (manager.frames.published().window?.generation ?? 0) >= carrying }
        try await waitUntil(seconds: 3) { heard.withLock { $0.contains { $0.generation == carrying } } }

        let histograms = heard.withLock { $0 }
        let window     = try #require(histograms.first { $0.generation == carrying })
        #expect(!histograms.contains { $0.generation == plain })
        #expect(window.isDraft)
        #expect(window.total == 512 * 341 || window.total == 512 * 342)
    }

    /// A render whose source is gone comes back through the C callback as a failure with the
    /// engine's message, for the request's generation, and publishes no frame.
    @Test
    func aRenderWhoseSourceIsGoneIsReportedNotSilent() async throws {
        let manager = try makeManager(maxPixels: 256)
        let file    = try fixtureURL()
        defer { try? FileManager.default.removeItem(at: file) }
        _ = try await manager.openPhoto(at: file)
        try FileManager.default.removeItem(at: file)

        let failures = Mutex<[FrameFailure]>([])
        manager.frames.onFailure = { failure in failures.withLock { $0.append(failure) } }

        let generation = try manager.requestPreview(
            maxPixels : 256,
            draft     : false,
            before    : false
        )

        try await waitUntil(seconds: 30) { !failures.withLock { $0 }.isEmpty }

        let failure = try #require(failures.withLock { $0 }.first)
        #expect(failure.generation == generation)
        #expect(!failure.isWindow)
        #expect(!failure.message.isEmpty)
        #expect(manager.frames.latest() == nil)
    }

    @Test
    func reopeningAfterTheFileMovedRelinksItAndStillRenders() async throws {
        let manager  = try makeManager(maxPixels: 256)
        let original = try fixtureURL()
        defer { try? FileManager.default.removeItem(at: original) }
        let bytes    = try Data(contentsOf: original)
        let first    = try await manager.openPhoto(at: original)

        // The reinstall case: the first copy is gone and the same bytes live at a new path.
        try FileManager.default.removeItem(at: original)
        let moved = FileManager.default.temporaryDirectory.appending(path: "moved-\(UUID()).png")
        try bytes.write(to: moved)
        defer { try? FileManager.default.removeItem(at: moved) }

        let second     = try await manager.openPhoto(at: moved)
        let generation = try manager.requestPreview(
            maxPixels : 256,
            draft     : false,
            before    : false
        )

        #expect(second == first)
        try await waitUntil(seconds: 30) { (manager.frames.latest()?.generation ?? 0) >= generation }
    }

    /// The everyday reopen: the same path again is a "path" duplicate, opened without a relink.
    @Test
    func reopeningTheSamePathSelectsTheSamePhotoAndRenders() async throws {
        let manager = try makeManager(maxPixels: 256)
        let file    = try fixtureURL()
        defer { try? FileManager.default.removeItem(at: file) }

        let first      = try await manager.openPhoto(at: file)
        let second     = try await manager.openPhoto(at: file)
        let generation = try manager.requestPreview(
            maxPixels : 256,
            draft     : false,
            before    : false
        )

        #expect(second == first)
        try await waitUntil(seconds: 30) { (manager.frames.latest()?.generation ?? 0) >= generation }
    }

    @Test
    func currentValuesReportsWhatWasSet() async throws {
        let manager = try makeManager(maxPixels: 256)
        let fixture = try fixtureURL()
        defer { try? FileManager.default.removeItem(at: fixture) }
        _ = try await manager.openPhoto(at: fixture)
        try await manager.set(.exposure, to: 0.5)

        let values = try await manager.currentValues()

        #expect(values[.exposure] == 0.5)
        #expect(values.count == DevelopAdjustmentKind.allCases.count)
    }

    @Test
    func editedFlagSizeAndResetsSpeakTheEngineJSON() async throws {
        let manager = try makeManager(maxPixels: 256)
        let fixture = try fixtureURL()
        defer { try? FileManager.default.removeItem(at: fixture) }
        let photo   = try await manager.openPhoto(at: fixture)

        let info    = try await manager.photoInfo(of: photo)
        #expect(info.size == CGSize(width: 96, height: 64))
        #expect(!info.isRaw)
        #expect(!info.hasEmbeddedLens)
        #expect(info.cameraDefaultsMerge.isEmpty)
        #expect(try await !manager.isEdited(photo))

        // Upstream `is_unedited` ignores white balance, so on a plain image only a non-WB
        // control makes the photo edited.
        try await manager.set(.exposure, to: 0.5)
        #expect(try await manager.isEdited(photo))

        try await manager.set(.tint, to: 20)
        try await manager.resetWhiteBalance()

        // On a plain image the raw look is an edit, which proves the merge reached the engine.
        let rawLook = DevelopMergeSettings(detail: PhotoInfo.rawDetail, optics: .init(lensProfile: true))
        try await manager.resetDevelop(photo, restoring: rawLook)

        let values = try await manager.currentValues()
        #expect(values[.exposure] == 0)
        #expect(values[.sharpening] == 40)
        #expect(values[.colorNoise] == 25)
        #expect(try await manager.isEdited(photo))

        try await manager.resetDevelop(photo, restoring: .none)
        #expect(try await !manager.isEdited(photo))
    }

    @Test
    func anUnknownFileIsAnErrorNotACrash() async throws {
        let manager = try makeManager(maxPixels: 256)
        let url     = FileManager.default.temporaryDirectory.appending(path: "broken-\(UUID()).ARW")
        try Data("not a raw".utf8).write(to: url)
        defer { try? FileManager.default.removeItem(at: url) }

        await #expect(throws: EngineError.self) {
            _ = try await manager.openPhoto(at: url)
        }
    }

    @Test
    func suspendedPreviewIsRefusedAndResumeRestoresPixels() async throws {
        let manager = try makeManager(maxPixels: 256)
        let fixture = try fixtureURL()
        defer { try? FileManager.default.removeItem(at: fixture) }
        _ = try await manager.openPhoto(at: fixture)

        let first = try manager.requestPreview(
            maxPixels : 256,
            draft     : false,
            before    : false
        )
        try await waitUntil(seconds: 30) { (manager.frames.latest()?.generation ?? 0) >= first }
        let before = try #require(manager.frames.latest())
        let buffer = manager.frames.buffer(at: before.bufferIndex)
        // Copy only pixel bytes while no further render can replace this published buffer.
        let pixels = (0 ..< before.height).map { row in
            Data(bytes: buffer.contents() + row * before.bytesPerRow, count: before.width * 4)
        }
        try manager.suspend()

        #expect(throws: EngineError.suspended("the session is suspended")) {
            _ = try manager.requestPreview(
                maxPixels : 256,
                draft     : false,
                before    : false
            )
        }
        try manager.resume()

        let resumed = try manager.requestPreview(
            maxPixels : 256,
            draft     : false,
            before    : false
        )
        try await waitUntil(seconds: 30) { (manager.frames.latest()?.generation ?? 0) >= resumed }
        let after = try #require(manager.frames.latest())
        #expect(after.width == before.width && after.height == before.height)
        #expect(after.nativeWidth == before.nativeWidth && after.nativeHeight == before.nativeHeight)
        #expect(!after.isDraft)
        let restored = manager.frames.buffer(at: after.bufferIndex)
        let restoredPixels = (0 ..< after.height).map { row in
            Data(bytes: restored.contents() + row * after.bytesPerRow, count: after.width * 4)
        }
        #expect(restoredPixels == pixels)
    }

    @Test
    func diagnosticAppGpuReturnsTheBackendState() async throws {
        let manager = try makeManager(maxPixels: 256)

        let json = try await manager.diagnostic("app.gpu", params: [String: String]())

        #expect(json.contains("\"enabled\""))
    }

    /// age dates each file an hour back, past the sweep's 10-minute grace period.
    func age(_ files: [URL]) throws {
        for file in files {
            try FileManager.default.setAttributes(
                [.modificationDate: Date.now.addingTimeInterval(-3600)],
                ofItemAtPath: file.path(percentEncoded: false)
            )
        }
    }

    /// The real catalog's answer decides the sweep: of three imported copies, the one whose photo
    /// is deleted from the catalog for good goes, the trashed one stays (a restore needs it) and
    /// so does the kept one.
    @Test
    func theSweepRemovesOnlyTheCopyOfAPhotoDeletedFromTheCatalog() async throws {
        let root = FileManager.default.temporaryDirectory.appending(path: "sweep-engine-\(UUID())")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let manager  = try makeManager(maxPixels: 256)
        let importer = PhotoImporter(importsDirectory: root.appending(path: "Imports"))
        let kept     = try importer.importCopy(of: fixtureURL(color: .systemTeal, in: root)).url
        let trashed  = try importer.importCopy(of: fixtureURL(color: .systemPink, in: root)).url
        let removed  = try importer.importCopy(of: fixtureURL(color: .systemOrange, in: root)).url

        let removedPhoto = try await manager.openPhoto(at: removed)
        let trashedPhoto = try await manager.openPhoto(at: trashed)
        _ = try await manager.openPhoto(at: kept)
        let removal = ["ids": [removedPhoto.rawValue]]
        let trash   = ["ids": [trashedPhoto.rawValue]]
        // In this order: both need a selection, and deletePermanently clears it.
        _ = try await manager.diagnostic("photo.delete", params: trash)
        _ = try await manager.diagnostic("photo.deletePermanently", params: removal)
        try age([kept, trashed, removed])

        let store  = ImportStore(catalog: manager, importsDirectory: importer.importsDirectory)
        let report = await store.sweep(keeping: nil)

        #expect(FileManager.default.fileExists(atPath: kept.path(percentEncoded: false)))
        #expect(FileManager.default.fileExists(atPath: trashed.path(percentEncoded: false)))
        #expect(!FileManager.default.fileExists(atPath: removed.path(percentEncoded: false)))
        #expect(report.removedCount == 1)
    }

    /// After an app update the container moves: the catalog still names the old folder, and the
    /// copy now lives under the same name in a new one. The name alone keeps it.
    @Test
    func aCopyTheCatalogNamesUnderAnotherContainerFolderStays() async throws {
        let root = FileManager.default.temporaryDirectory.appending(path: "sweep-moved-\(UUID())")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: root) }

        let manager     = try makeManager(maxPixels: 256)
        let oldImports  = root.appending(path: "OldContainer/Imports", directoryHint: .isDirectory)
        let newImports  = root.appending(path: "NewContainer/Imports", directoryHint: .isDirectory)
        let oldImporter = PhotoImporter(importsDirectory: oldImports)
        let oldCopy     = try oldImporter.importCopy(of: fixtureURL(in: root)).url
        _ = try await manager.openPhoto(at: oldCopy)

        try FileManager.default.createDirectory(at: newImports, withIntermediateDirectories: true)
        let movedCopy = newImports.appending(path: oldCopy.lastPathComponent)
        let orphan    = newImports.appending(path: "0000-orphan.png")
        try FileManager.default.copyItem(at: oldCopy, to: movedCopy)
        try Data([1, 2, 3]).write(to: orphan)
        try age([movedCopy, orphan])

        let store  = ImportStore(catalog: manager, importsDirectory: newImports)
        let report = await store.sweep(keeping: nil)

        #expect(FileManager.default.fileExists(atPath: movedCopy.path(percentEncoded: false)))
        #expect(!FileManager.default.fileExists(atPath: orphan.path(percentEncoded: false)))
        #expect(report.removedCount == 1)
    }
}
