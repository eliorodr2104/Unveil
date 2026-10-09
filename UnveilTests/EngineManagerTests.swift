//
//  EngineManagerTests.swift
//  UnveilTests
//

import Foundation
import Testing
import UIKit
@testable import Unveil

/// Serialized: the engine's GPU switch, memory budget and environment are process-wide,
/// and each manager gets its own library folder because a library is file-locked.
@Suite(.serialized)
struct EngineManagerTests {

    /// A PNG the engine can import: the simulator has no RAW samples.
    func fixtureURL() throws -> URL {
        let url      = FileManager.default.temporaryDirectory.appending(path: "fixture-\(UUID()).png")
        let renderer = UIGraphicsImageRenderer(size: CGSize(width: 96, height: 64))
        let data     = renderer.pngData { context in
            UIColor.systemTeal.setFill()
            context.fill(CGRect(x: 0, y: 0, width: 96, height: 64))
        }

        try data.write(to: url)
        return url
    }

    func makeManager(maxPixels: Int) throws -> EngineManager {
        let library = FileManager.default.temporaryDirectory.appending(path: "engine-\(UUID())")
        return try EngineManager(dataDirectory: library, maxPixels: maxPixels)
    }

    @Test
    func openSetAndPreviewDeliversAFrame() async throws {
        let manager = try makeManager(maxPixels: 512)
        _ = try await manager.openPhoto(at: fixtureURL())
        try await manager.set(.exposure, to: 0.5)

        let generation = try manager.requestPreview(maxPixels: 512, draft: false)

        // A newer generation may overtake this one, so any frame at or above it counts.
        try await waitUntil(seconds: 30) { (manager.frames.latest()?.generation ?? 0) >= generation }
    }

    @Test
    func currentValuesReportsWhatWasSet() async throws {
        let manager = try makeManager(maxPixels: 256)
        _ = try await manager.openPhoto(at: fixtureURL())
        try await manager.set(.exposure, to: 0.5)

        let values = try await manager.currentValues()

        #expect(values[.exposure] == 0.5)
        #expect(values.count == DevelopAdjustmentKind.allCases.count)
    }

    @Test
    func anUnknownFileIsAnErrorNotACrash() async throws {
        let manager = try makeManager(maxPixels: 256)
        let url     = FileManager.default.temporaryDirectory.appending(path: "broken-\(UUID()).ARW")
        try Data("not a raw".utf8).write(to: url)

        await #expect(throws: EngineError.self) {
            _ = try await manager.openPhoto(at: url)
        }
    }

    @Test
    func previewWhileSuspendedIsRefused() async throws {
        let manager = try makeManager(maxPixels: 256)
        _ = try await manager.openPhoto(at: fixtureURL())
        try manager.suspend()

        #expect(throws: EngineError.suspended("the session is suspended")) {
            _ = try manager.requestPreview(maxPixels: 256, draft: false)
        }
        try manager.resume()
    }

    @Test
    func diagnosticAppGpuReturnsTheBackendState() async throws {
        let manager = try makeManager(maxPixels: 256)

        let json = try await manager.diagnostic("app.gpu", params: [String: String]())

        #expect(json.contains("\"enabled\""))
    }
}
