//
//  EditorViewModel.swift
//  Unveil
//

import Foundation
import Observation
import os

/// EditorViewModel is the state of the editor: which photo is open, the ten slider values and the
/// last error to show. The view reads it, and it drives the engine through `EngineDriving`.
///
/// Every engine call goes through one chain, `pending`: each step awaits the one before it. That is
/// what makes a full preview on release safe. The engine's `set` is queued asynchronously while
/// `requestPreview` is immediate, so without the chain the release preview could overtake the last
/// drag's `set` and render a frame that lacks the final value. Values are clamped to the kind's
/// range here, so the slider and the engine never disagree. Errors never propagate: they land in
/// `errorMessage` for the view to show.
///
/// A slider at 120 Hz is faster than the engine's round trip, so drags are coalesced: each kind
/// keeps only its newest wanted value and at most one drag link waits in the chain. When the link
/// runs it sends the newest value (and its draft preview), or nothing if that value was already
/// sent, so the backlog never grows and the engine never sees an older value after a newer one.
@Observable
@MainActor
final class EditorViewModel<Engine: EngineDriving, Importer: PhotoImporting> {

    private(set) var values      : [DevelopAdjustmentKind: Double]
    private(set) var openPhoto   : PhotoID?
    private(set) var errorMessage: String?

    private let engine       : Engine
    private let importer     : Importer
    private let previewPixels: Int

    // Plain bookkeeping the view never reads, so it stays out of observation.
    @ObservationIgnored
    private var pending: Task<Void, Never>?

    @ObservationIgnored
    private var wanted: [DevelopAdjustmentKind: Double] = [:]

    @ObservationIgnored
    private var sent: [DevelopAdjustmentKind: Double] = [:]

    @ObservationIgnored
    private var waitingDrags: Set<DevelopAdjustmentKind> = []

    /// isPhotoOpen is false until `open` succeeds: the view keeps the sliders disabled until then.
    var isPhotoOpen: Bool { openPhoto != nil }

    init(engine: Engine, importer: Importer, previewPixels: Int) {
        self.engine        = engine
        self.importer      = importer
        self.previewPixels = previewPixels
        self.values        = Self.defaultValues
    }

    /// open copies the picked file off the main actor, opens it in the engine, loads the controls'
    /// current values into the sliders and shows a full preview. A photo already in the library
    /// reopens with its saved settings, so the sliders show what the engine holds, not the defaults.
    ///
    /// An open that fails, even halfway (the import worked, the select or the values read did not),
    /// leaves both sides on the previous state: the model keeps its photo and values, and the engine
    /// is told to select that photo again, which also undoes a select that timed out but still runs,
    /// because the engine takes commands in order. A copy made for this open is deleted. With no
    /// previous photo the model stays closed, so nothing acts on whatever the engine has selected.
    ///
    /// It is the `OpenToFirstFrame` signpost's start; the interval ends when the canvas presents the
    /// preview asked for here, or at once as `failed` when no preview was asked for.
    func open(pickedURL: URL) async {
        let signpost = Signposts.beginOpen()
        let copy: (url: URL, isNew: Bool)

        do {
            copy = try await Self.importCopy(of: pickedURL, using: importer)
        } catch {
            Signposts.openRequestedPreview(signpost, generation: nil)
            errorMessage = Self.message(for: error)
            return
        }

        await enqueue { [engine, previewPixels] in
            var previewGeneration: UInt64?
            defer { Signposts.openRequestedPreview(signpost, generation: previewGeneration) }

            let previous = self.openPhoto
            let photo    : PhotoID
            let current  : [DevelopAdjustmentKind: Double]

            do {
                photo   = try await engine.openPhoto(at: copy.url)
                current = try await engine.currentValues()
            } catch {
                await Self.rollBack(engine: engine, to: previous, discarding: copy)
                throw error
            }

            self.openPhoto = photo
            self.values    = Self.defaultValues.merging(current) { _, engineValue in engineValue }
            self.sent      = self.values
            self.wanted    = [:]

            previewGeneration = try engine.requestPreview(maxPixels: previewPixels, draft: false)
        }
    }

    /// beginDrag marks the start of a slider gesture. There is nothing to prepare: `drag` does all
    /// the work. It exists so the view calls begin, drag and end symmetrically.
    func beginDrag(_ kind: DevelopAdjustmentKind) {}

    /// drag clamps the value, shows it at once and records it as the newest wanted value for its
    /// kind. If a drag link for that kind is already waiting in the chain it will pick this value up
    /// and there is nothing more to queue; otherwise one link is queued behind whatever is in flight.
    func drag(_ kind: DevelopAdjustmentKind, to value: Double) async {
        let clamped = min(max(value, kind.range.lowerBound), kind.range.upperBound)
        values[kind] = clamped
        wanted[kind] = clamped

        guard waitingDrags.insert(kind).inserted else { return }

        await enqueue { [engine, previewPixels] in
            self.waitingDrags.remove(kind)

            guard let newest = self.wanted[kind], self.sent[kind] != newest else { return }

            try await engine.set(kind, to: newest)
            self.sent[kind] = newest

            _ = try engine.requestPreview(maxPixels: previewPixels, draft: true)
        }
    }

    /// endDrag queues the final `set` (only if the engine has not got that value yet) and then the
    /// full-quality preview, behind every earlier link, so the final frame carries the final value.
    func endDrag(_ kind: DevelopAdjustmentKind) async {
        await enqueue { [engine, previewPixels] in
            if let newest = self.wanted[kind], self.sent[kind] != newest {
                try await engine.set(kind, to: newest)
                self.sent[kind] = newest
            }

            _ = try engine.requestPreview(maxPixels: previewPixels, draft: false)
        }
    }

    /// refreshPreview asks for one full preview of the open photo, behind any queued work. The app
    /// calls it when the engine resumes: a render refused while suspended would otherwise leave the
    /// canvas on a draft frame until the next slider move.
    func refreshPreview() async {
        guard isPhotoOpen else { return }

        await enqueue { [engine, previewPixels] in
            _ = try engine.requestPreview(maxPixels: previewPixels, draft: false)
        }
    }

    func dismissError() {
        errorMessage = nil
    }

    // MARK: - Chain

    /// enqueue appends `step` to the chain and waits for it. The task is created before the first
    /// suspension, so steps run in the order the callers reached this point.
    private func enqueue(_ step: @escaping @MainActor () async throws -> Void) async {
        let previous = pending

        let task = Task {
            await previous?.value

            do {
                try await step()
            } catch EngineError.suspended {
                // Expected when the app resigns active mid-drag: refreshPreview redraws on resume.
            } catch let error as EngineError {
                errorMessage = error.message
            } catch {
                errorMessage = error.localizedDescription
            }
        }

        pending = task
        await task.value
    }

    // MARK: - Helpers

    /// rollBack puts the engine back on `previous` and deletes `copy` if this open made it. Its own
    /// failures are only logged: the open's error is the one the user sees.
    private static func rollBack(
        engine          : Engine,
        to previous     : PhotoID?,
        discarding copy : (url: URL, isNew: Bool)
    ) async {
        let logger = Logger(subsystem: "com.eliorodr2104.unveil", category: "EditorViewModel")

        if let previous {
            do {
                try await engine.select(previous)
            } catch {
                logger.error("Re-selecting the previous photo failed: \(error.message, privacy: .public)")
            }
        }

        if copy.isNew {
            do {
                try FileManager.default.removeItem(at: copy.url)
            } catch {
                logger.error("Deleting a failed import failed: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    private static var defaultValues: [DevelopAdjustmentKind: Double] {
        Dictionary(uniqueKeysWithValues: DevelopAdjustmentKind.allCases.map { ($0, $0.defaultValue) })
    }

    /// importCopy runs the importer off the main actor: copying a 50 to 100 MB RAW would stall the UI.
    /// The copy is the `ImportCopy` signpost.
    @concurrent
    private static func importCopy(
        of pickedURL   : URL,
        using importer : Importer
    ) async throws(PhotoImportError) -> (url: URL, isNew: Bool) {
        let signpost = Signposts.signposter.beginInterval(
            "ImportCopy",
            id: Signposts.signposter.makeSignpostID()
        )
        defer { Signposts.signposter.endInterval("ImportCopy", signpost) }

        return try importer.importCopy(of: pickedURL)
    }

    private static func message(for error: PhotoImportError) -> String {
        switch error {
            case .accessDenied(let url):
                return "The app may not read \(url.lastPathComponent)."

            case .copyFailed(let text):
                return "The file could not be copied: \(text)"
        }
    }
}
