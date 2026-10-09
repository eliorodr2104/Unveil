//
//  EditorViewModelTests.swift
//  UnveilTests
//

import Foundation
import Testing
@testable import Unveil

@MainActor
struct EditorViewModelTests {

    @Test
    func dragSendsValueThenDraftPreviewAndReleaseSendsFull() async {
        let engine = FakeEngine()
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)

        model.beginDrag(.exposure)
        await model.drag(.exposure, to: 0.7)
        await model.endDrag(.exposure)

        #expect(engine.calls == ["set light.exposure 0.7", "preview 1024 draft", "preview 1024 full"])
    }

    @Test
    func valuesAreClampedToTheControlRange() async {
        let engine = FakeEngine()
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)

        await model.drag(.exposure, to: 99)

        #expect(model.values[.exposure] == DevelopAdjustmentKind.exposure.range.upperBound)
    }

    @Test
    func anEngineErrorBecomesAMessageNotACrash() async {
        let engine = FakeEngine()
        engine.failNextOpen = .engine("unsupported camera")
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)

        await model.open(pickedURL: URL(filePath: "/tmp/x.ARW"))

        #expect(model.errorMessage?.contains("unsupported camera") == true)
        #expect(model.openPhoto == nil)
    }

    @Test
    func openingAPhotoShowsAFullPreviewAndDismissClearsTheError() async {
        let engine = FakeEngine()
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)

        await model.open(pickedURL: URL(filePath: "/tmp/x.ARW"))

        #expect(model.openPhoto == PhotoID(rawValue: 1))
        #expect(engine.calls == ["open /tmp/x.ARW", "preview 1024 full"])

        engine.failNextOpen = .io("disk full")
        await model.open(pickedURL: URL(filePath: "/tmp/y.ARW"))
        #expect(model.errorMessage == "disk full")

        model.dismissError()
        #expect(model.errorMessage == nil)
    }

    @Test
    func openingAPhotoLoadsTheValuesTheEngineHolds() async {
        let engine = FakeEngine()
        engine.storedValues = [.exposure: 1.5]
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)

        #expect(model.isPhotoOpen == false)

        await model.open(pickedURL: URL(filePath: "/tmp/x.ARW"))

        #expect(model.isPhotoOpen)
        #expect(model.values[.exposure] == 1.5)
        #expect(model.values[.temperature] == DevelopAdjustmentKind.temperature.defaultValue)
    }

    /// The engine opened the new photo but its values could not be read: the model keeps the first
    /// photo and its values, and the engine is put back on it, with no preview of the new one.
    @Test
    func anOpenThatFailsHalfwayLeavesBothOnThePreviousPhoto() async {
        let engine = FakeEngine()
        engine.storedValues = [.exposure: 1.5]
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)

        await model.open(pickedURL: URL(filePath: "/tmp/x.ARW"))
        engine.failNextValues = .timeout("develop.controls timed out")
        engine.storedValues   = [.exposure: -2]
        await model.open(pickedURL: URL(filePath: "/tmp/y.ARW"))

        #expect(model.openPhoto == PhotoID(rawValue: 1))
        #expect(model.values[.exposure] == 1.5)
        #expect(model.errorMessage?.contains("timed out") == true)
        #expect(engine.calls == ["open /tmp/x.ARW", "preview 1024 full", "open /tmp/y.ARW", "select 1"])
    }

    /// A failed open must not leave its copy behind: the real importer, on a folder of its own.
    @Test
    func aFailedOpenLeavesNoCopy() async throws {
        let imports = FileManager.default.temporaryDirectory.appending(path: "imports-\(UUID())")
        let picked  = FileManager.default.temporaryDirectory.appending(path: "picked-\(UUID()).ARW")
        try Data(UUID().uuidString.utf8).write(to: picked)

        let engine = FakeEngine()
        engine.failNextOpen = .engine("unsupported camera")
        let model  = EditorViewModel(
            engine        : engine,
            importer      : PhotoImporter(importsDirectory: imports),
            previewPixels : 1024
        )

        await model.open(pickedURL: picked)

        #expect(model.openPhoto == nil)
        #expect(try FileManager.default.contentsOfDirectory(atPath: imports.path(percentEncoded: false)).isEmpty)
    }

    /// The drags and the release are started without awaiting each other, like a finger that moves
    /// faster than the engine. FakeEngine.set yields, so an unchained release would run ahead, and
    /// the stale drags must collapse into far fewer than 40 `set` calls.
    @Test
    func rapidDragsThenReleaseEndWithTheFinalValueBeforeTheFullPreview() async {
        let engine = FakeEngine()
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)
        var tasks  = [Task<Void, Never>]()

        for step in 1 ... 40 {
            let value = Double(step) / 10
            tasks.append(Task { await model.drag(.exposure, to: value) })
        }
        tasks.append(Task { await model.endDrag(.exposure) })

        for task in tasks {
            await task.value
        }

        let calls = engine.calls
        let sets  = calls.filter { $0.hasPrefix("set ") }

        #expect(sets.count < 20)
        #expect(sets.last == "set light.exposure 4.0")
        #expect(calls.last == "preview 1024 full")
        #expect(calls.firstIndex(of: "preview 1024 full") == calls.count - 1)
        #expect(model.values[.exposure] == 4.0)
    }

    /// Drags are interleaved with yields so some links run in the middle of the gesture: the engine
    /// then receives several values, and none of them may be older than the one before it.
    @Test
    func theEngineNeverReceivesAnOlderValueAfterANewerOne() async {
        let engine = FakeEngine()
        let model  = EditorViewModel(engine: engine, importer: FakeImporter(), previewPixels: 1024)
        var tasks  = [Task<Void, Never>]()

        for step in 1 ... 60 {
            let value = Double(step) / 20
            tasks.append(Task { await model.drag(.exposure, to: value) })
            await Task.yield()
        }
        tasks.append(Task { await model.endDrag(.exposure) })

        for task in tasks {
            await task.value
        }

        let sent = engine.calls
            .filter { $0.hasPrefix("set light.exposure ") }
            .compactMap { Double($0.dropFirst("set light.exposure ".count)) }

        #expect(sent.count < 60)
        #expect(sent == sent.sorted())
        #expect(sent.last == 3.0)
        #expect(engine.calls.last == "preview 1024 full")
    }
}
