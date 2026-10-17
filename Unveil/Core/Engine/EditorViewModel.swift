//
//  EditorViewModel.swift
//  Unveil
//

import CoreGraphics
import Foundation
import Observation
import os
import UniformTypeIdentifiers

/// EditorViewModel is the state of the editor: which photo is open, the slider values and the
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
    private(set) var openCopyURL : URL?

    /// errorMessages are the errors raised and not yet acknowledged, oldest first: a second error
    /// while the first is on screen waits behind it. `errorMessage` is the one to show now.
    private(set) var errorMessages: [String] = []

    /// renderErrorMessage is the last failed preview of the open photo, for a non-blocking banner:
    /// the canvas keeps its last good frame and editing goes on. A new preview request clears it.
    private(set) var renderErrorMessage: String?
    private(set) var photoTitle  : String = "Photo"
    private(set) var photoType   : String?

    /// histogram is the newest histogram of the whole open photo; nil until the first frame of a newly
    /// opened photo. At fit every draft of a drag updates it; zoomed in, a draft window carries it at
    /// most every `histogramInterval` (from a small whole frame the engine renders after the window),
    /// so it follows the drag at about a dozen updates a second without a preview render.
    private(set) var histogram: FrameHistogram?

    /// isShowingBefore is true while the canvas shows the photo's unedited look (a press and hold, or
    /// the backslash toggle). Every frame asked for in the meantime is a before frame, the edited one
    /// stays on screen until the first arrives, and the sliders and edit commands are off.
    private(set) var isShowingBefore = false

    /// captureSummary is the open photo's exposure as recorded ("ISO 200 · 1/250 s · f/2.8 · 35 mm"),
    /// nil when the file records none.
    private(set) var captureSummary: String?

    /// isEdited is the editor's "Edited" status: the engine's own flag (`Photo::is_edited`, which
    /// compares with the photo's import look), read after open and after every committed change.
    /// A drag does not read it: the status catches up when the slider is released.
    private(set) var isEdited = false

    /// undoStack and redoStack are the snapshots from before each committed edit, newest last. Undo
    /// takes one off the first, redo off the second; both are per open photo and in memory.
    private(set) var undoStack: [EditSnapshot] = []
    private(set) var redoStack: [EditSnapshot] = []

    var errorMessage: String? { errorMessages.first }

    var canUndo: Bool { !undoStack.isEmpty }
    var canRedo: Bool { !redoStack.isEmpty }

    /// photoSize is the open photo's size in real pixels, `.zero` when unknown. The zoom pill
    /// measures against it, not against the preview.
    private(set) var photoSize: CGSize = .zero

    /// developDefaults is each control's value right after import for the open photo (see
    /// `PhotoInfo.developDefaults`): a raw starts with sharpening and color noise reduction.
    private(set) var developDefaults: [DevelopAdjustmentKind: Double]

    /// cameraDefaults is what Reset All merges back after `develop.reset` (see
    /// `PhotoInfo.cameraDefaultsMerge`).
    @ObservationIgnored
    private var cameraDefaults = DevelopMergeSettings.none

    /// maxUndoSteps caps the undo stack: past it the oldest step is dropped.
    static var maxUndoSteps: Int { 100 }

    private let engine            : Engine
    private let importer          : Importer
    private let previewPixels     : Int
    private let settleNudge       : @Sendable () async throws -> Void
    private let windowReleaseDelay: Duration

    // Plain bookkeeping the view never reads, so it stays out of observation.
    @ObservationIgnored
    private var pending: Task<Bool, Never>?

    /// newestPreviewGeneration is the generation of the newest whole-frame preview asked for: a
    /// failure of an older one says nothing about what the canvas shows now.
    @ObservationIgnored
    private var newestPreviewGeneration: UInt64 = 0

    /// histogramFloor is the frame sink's floor at the last open: a histogram at or below it belongs
    /// to the photo shown before, heard just before the open and delivered just after.
    @ObservationIgnored
    private var histogramFloor: UInt64 = 0

    /// histogramInterval is the least time between the draft windows that ask for the histogram
    /// while a drag renders windows: each one costs the engine a small whole frame besides.
    /// `now` is the clock it is measured on; tests drive it.
    @ObservationIgnored
    private let histogramInterval: Duration

    @ObservationIgnored
    private let now: () -> ContinuousClock.Instant

    /// lastWindowHistogram is when the newest of those windows was asked for.
    @ObservationIgnored
    private var lastWindowHistogram: ContinuousClock.Instant?

    @ObservationIgnored
    private var wanted: [DevelopAdjustmentKind: Double] = [:]

    @ObservationIgnored
    private var sent: [DevelopAdjustmentKind: Double] = [:]

    /// sentAsShot is whether the engine's white balance mode is As Shot: read at open and at a
    /// reread, false after any set of temperature or tint, true after a white balance reset.
    @ObservationIgnored
    private var sentAsShot = true

    /// committed is what the engine held at the last commit, undo or redo: the snapshot the next
    /// committed edit pushes on the undo stack.
    @ObservationIgnored
    private var committed = EditSnapshot(values: [:], isWhiteBalanceAsShot: true)

    @ObservationIgnored
    private var waitingDrags: Set<DevelopAdjustmentKind> = []

    /// visibleWindow is the window of the zoomed frame the canvas shows over the preview, or nil
    /// while the preview is enough (see `showWindow`). Edits render it along with the preview.
    @ObservationIgnored
    private var visibleWindow: FrameWindow?

    /// windowRelease is the timer that frees the engine's window memory once the canvas has wanted
    /// no window for `windowReleaseDelay` (see `showWindow`).
    @ObservationIgnored
    private var windowRelease: Task<Void, Never>?

    /// nudgeSettle is the stepper burst waiting to be committed: its kind and the timer that
    /// commits it once the presses stop.
    @ObservationIgnored
    private var nudgeSettle: (kind: DevelopAdjustmentKind, timer: Task<Void, Never>)?

    /// isPhotoOpen is false until `open` succeeds: the view keeps the sliders disabled until then.
    var isPhotoOpen: Bool { openPhoto != nil }

    /// defaultValue(of:) is the open photo's own default for `kind`, what a single control's reset
    /// goes back to and what the slider styles as "at default".
    func defaultValue(of kind: DevelopAdjustmentKind) -> Double {
        developDefaults[kind] ?? kind.defaultValue
    }

    /// `settleNudge` is the wait after the last stepper press before the burst is committed (0.3 s;
    /// tests pass a wait they end themselves, so a burst never splits under load), and
    /// `windowReleaseDelay` how long the canvas must want no window before the engine frees the
    /// windows' memory; tests shorten it. `histogramInterval` and `now` pace the draft windows that
    /// carry the histogram while a window is shown; tests drive the clock.
    init(
        engine            : Engine,
        importer          : Importer,
        previewPixels     : Int,
        settleNudge       : @escaping @Sendable () async throws -> Void = {
            try await Task.sleep(for: .milliseconds(300))
        },
        windowReleaseDelay: Duration = .seconds(2),
        histogramInterval : Duration = .milliseconds(80),
        now               : @escaping () -> ContinuousClock.Instant = { .now }
    ) {
        self.engine             = engine
        self.importer           = importer
        self.previewPixels      = previewPixels
        self.settleNudge        = settleNudge
        self.windowReleaseDelay = windowReleaseDelay
        self.histogramInterval  = histogramInterval
        self.now                = now
        self.values             = Self.defaultValues
        self.developDefaults    = Self.defaultValues

        engine.frames.onFailure = { [weak self] failure in
            Task { @MainActor in self?.show(failure) }
        }

        engine.frames.onHistogram = { [weak self] histogram in
            Task { @MainActor in self?.show(histogram) }
        }
    }

    /// open copies the picked file off the main actor, opens it in the engine, loads the controls'
    /// current values into the sliders and shows a full preview. A photo already in the library
    /// reopens with its saved settings, so the sliders show what the engine holds, not the defaults.
    ///
    /// An open that fails, even halfway (the import worked, the select or the values read did not),
    /// leaves both sides on the previous state: the model keeps its photo and values, and the engine
    /// is told to select that photo again, which also undoes a select that timed out but still runs,
    /// because the engine takes commands in order. A copy made for this open is deleted only if the
    /// engine never returned a photo for it: after that its record points at the copy. With no
    /// previous photo the model stays closed, so nothing acts on whatever the engine has selected.
    ///
    /// A successful open also sets `photoTitle` (the picked file's name without its extension, or
    /// `title` when the caller knows a better one), `photoType` and `photoSize` for the Info sheet,
    /// the photo's develop defaults and the engine's edited flag.
    ///
    /// It is the `OpenToFirstFrame` signpost's start; the interval ends when the canvas presents the
    /// preview asked for here, or at once as `failed` when no preview was asked for.
    ///
    /// It returns true once the new photo is open in both the model and the engine. A first preview
    /// refused after that does not undo the open: the editor shows the photo's alert, or nothing
    /// for `.suspended`, since `refreshPreview` draws the photo when the engine resumes.
    ///
    /// A stepper burst still waiting for its timer is committed first, on the photo it edited.
    @discardableResult
    func open(pickedURL: URL, title: String? = nil) async -> Bool {
        commitPendingNudge()

        let signpost = Signposts.beginOpen()
        OpenTimeline.mark("open")
        let copy: (url: URL, isNew: Bool)

        do {
            copy = try await Self.importCopy(of: pickedURL, using: importer)
        } catch {
            Signposts.openRequestedPreview(signpost, generation: nil)
            report(Self.message(for: error))
            return false
        }

        return await enqueue { [engine] in
            var previewGeneration: UInt64?
            defer { Signposts.openRequestedPreview(signpost, generation: previewGeneration) }

            let previous = self.openPhoto
            let photo    : PhotoID
            let current  : [DevelopAdjustmentKind: Double]
            let info     : PhotoInfo
            let edited   : Bool
            let asShot   : Bool

            // The copy is deleted only if no photo came back: once the engine has an id its record
            // points at the copy (a new import, or an existing photo relinked to it).
            do {
                photo = try await engine.openPhoto(at: copy.url)
            } catch {
                await Self.rollBack(engine: engine, to: previous, discarding: copy.isNew ? copy.url : nil)
                throw error
            }

            do {
                current = try await engine.currentValues()
                info    = try await engine.photoInfo(of: photo)
                edited  = try await engine.isEdited(photo)
                asShot  = try await engine.isWhiteBalanceAsShot()
            } catch {
                await Self.rollBack(engine: engine, to: previous, discarding: nil)
                throw error
            }

            self.openPhoto       = photo
            self.openCopyURL     = copy.url
            self.visibleWindow   = nil
            self.values          = Self.defaultValues.merging(current) { _, engineValue in engineValue }
            self.sent            = self.values
            self.wanted          = [:]
            self.sentAsShot      = asShot
            self.committed       = self.current
            self.undoStack       = []
            self.redoStack       = []
            self.isEdited        = edited
            self.photoSize       = info.size
            self.developDefaults = info.developDefaults
            self.cameraDefaults  = info.cameraDefaultsMerge
            self.captureSummary  = info.capture?.summary
            self.histogram       = nil
            self.isShowingBefore = false

            self.photoTitle = title ?? pickedURL.deletingPathExtension().lastPathComponent
            self.photoType  = UTType(filenameExtension: pickedURL.pathExtension)?.localizedDescription

            // The previous photo's frame must not show while this one renders, nor its histogram.
            self.histogramFloor = engine.frames.invalidate()

            do throws(EngineError) {
                previewGeneration = try self.requestPreview(draft: false)
                OpenTimeline.mark("preview-requested")
            } catch .suspended {
                // The app left mid-open: refreshPreview redraws on resume, so there is nothing to say.
            } catch {
                self.report(error.message)
            }
        }
    }

    /// beginDrag marks the start of a slider gesture (or of a whole edit such as a value entry).
    /// `drag` does the work; the only thing to prepare is committing a stepper burst still waiting
    /// for its timer, so the burst and the new edit stay two separate edits.
    func beginDrag(_ kind: DevelopAdjustmentKind) {
        commitPendingNudge()
    }

    /// nudge moves `kind` by `delta` from a stepper press, snapped to the step and the range. Every
    /// press of a burst (a hold repeats it) sends a draft through `drag`; the burst is committed
    /// once, `settleNudge` after the last press, by one `endDrag`: the burst's commit point.
    /// A press on another control first commits the burst in progress.
    func nudge(_ kind: DevelopAdjustmentKind, by delta: Double) async {
        if let pending = nudgeSettle, pending.kind != kind {
            commitPendingNudge()
        }

        nudgeSettle?.timer.cancel()

        let timer = Task { [weak self, settleNudge] in
            do {
                try await settleNudge()
            } catch {
                return
            }

            // A press that lands after the sleep ended but before this resumes still cancels it.
            guard !Task.isCancelled, let self else { return }

            self.nudgeSettle = nil
            await self.endDrag(kind)
        }
        nudgeSettle = (kind, timer)

        await drag(kind, to: kind.snapped((values[kind] ?? kind.defaultValue) + delta))
    }

    /// drag clamps the value, shows it at once and records it as the newest wanted value for its
    /// kind. If a drag link for that kind is already waiting in the chain it will pick this value up
    /// and there is nothing more to queue; otherwise one link is queued behind whatever is in flight.
    func drag(_ kind: DevelopAdjustmentKind, to value: Double) async {
        let clamped = min(max(value, kind.range.lowerBound), kind.range.upperBound)
        values[kind] = clamped
        wanted[kind] = clamped

        guard waitingDrags.insert(kind).inserted else { return }

        await enqueue { [engine] in
            self.waitingDrags.remove(kind)

            guard let newest = self.wanted[kind], self.sent[kind] != newest else { return }

            try await engine.set(kind, to: newest)
            self.sent[kind] = newest
            self.noteSet(of: kind)

            try self.requestFrames(draft: true)
        }
    }

    /// endDrag queues the final `set` (only if the engine has not got that value yet) and then the
    /// full-quality preview, behind every earlier link, so the final frame carries the final value.
    /// The edited flag is read after the preview is asked for, so the render starts first.
    func endDrag(_ kind: DevelopAdjustmentKind) async {
        await enqueue { [engine] in
            if let newest = self.wanted[kind], self.sent[kind] != newest {
                try await engine.set(kind, to: newest)
                self.sent[kind] = newest
                self.noteSet(of: kind)
            }

            self.commitEdit()

            try self.requestFrames(draft: false)
            try await self.reloadEdited()
        }
    }

    /// resetAll puts the open photo back to its import look: `develop.reset` (white balance As Shot,
    /// every other control at its default), then one `develop.merge` of what `camera_defaults` adds
    /// (raw detail, embedded lens corrections). The reread runs even when the merge failed, since
    /// the reset has landed by then, and so does the one full preview; the merge's error follows.
    /// Wanted drag values are dropped, so a drag link still waiting in the chain sends nothing
    /// stale after the reset. A waiting stepper burst is committed first, as its own edit.
    func resetAll() async {
        commitPendingNudge()

        guard isEdited, let photo = openPhoto else { return }

        wanted = [:]

        await enqueue { [engine, cameraDefaults] in
            var resetError: EngineError?

            do throws(EngineError) {
                try await engine.resetDevelop(photo, restoring: cameraDefaults)
            } catch {
                resetError = error
            }

            try await self.reloadValues()
            self.commitEdit()
            try self.requestFrames(draft: false)

            if let resetError {
                throw resetError
            }
        }
    }

    /// reset(_:) puts one control back to its default as a whole edit. Temperature and tint go back
    /// to As Shot together (`develop.wb`), since their default is the photo's, not a constant.
    /// Either way a waiting stepper burst is committed first (`beginDrag` does it for the others).
    func reset(_ kind: DevelopAdjustmentKind) async {
        guard kind.isWhiteBalance else {
            beginDrag(kind)
            await drag(kind, to: defaultValue(of: kind))
            await endDrag(kind)
            return
        }

        commitPendingNudge()

        wanted[.temperature] = nil
        wanted[.tint]        = nil

        await enqueue { [engine] in
            try await engine.resetWhiteBalance()
            try await self.reloadValues()
            self.commitEdit()

            try self.requestFrames(draft: false)
        }
    }

    /// undo goes back to the snapshot before the last committed edit, as `apply` does, and keeps
    /// the state it left on the redo stack. A waiting stepper burst is committed first.
    func undo() async {
        await step(from: \.undoStack, to: \.redoStack)
    }

    /// redo reapplies the last undone edit. A new edit clears what is left to redo.
    func redo() async {
        await step(from: \.redoStack, to: \.undoStack)
    }

    /// refreshPreview asks for one full preview of the open photo, behind any queued work. The app
    /// calls it when the engine resumes: a render refused while suspended would otherwise leave the
    /// canvas on a draft frame until the next slider move.
    func refreshPreview() async {
        guard isPhotoOpen else { return }

        await enqueue {
            try self.requestFrames(draft: false)
        }
    }

    /// setShowingBefore switches the canvas between the unedited look and the edit. The flag flips
    /// at once, so the capsule and the disabled sliders follow the finger, and one full preview (and
    /// the window, when zoomed) is asked for behind any queued edit, in the look the flag has when
    /// it runs. A stepper burst still waiting for its timer is committed first.
    func setShowingBefore(_ isShowing: Bool) async {
        guard isShowing != isShowingBefore, isPhotoOpen || !isShowing else { return }

        commitPendingNudge()
        isShowingBefore = isShowing

        await enqueue {
            try self.requestFrames(draft: false)
        }
    }

    /// showWindow records which window of the zoomed frame the canvas shows over the preview (nil:
    /// the preview is enough, and edits render only the preview again) and asks for it, behind any
    /// queued edit, so the window carries the newest settings. The canvas calls it as the view moves:
    /// at most every 100 ms during a gesture with `draft`, and once at full quality at rest.
    ///
    /// A window retains its full GPU original and render stages. Once nil has held for
    /// `windowReleaseDelay`, `releaseWindows` drops the window stages and the full source unless
    /// the fitted preview still uses it. A later full zoom decodes again after that release.
    /// Any window in between cancels the delayed release, so a quick zoom out and in keeps the
    /// existing source. The separate idle GPU-pool trim frees only reusable scratch buffers.
    /// The release goes straight to the engine: it orders nothing against the edits in the chain.
    func showWindow(_ window: FrameWindow?, draft: Bool) async {
        visibleWindow = window
        windowRelease?.cancel()
        windowRelease = nil

        guard isPhotoOpen else { return }

        guard let window else {
            windowRelease = Task { [weak self, engine, windowReleaseDelay] in
                do {
                    try await Task.sleep(for: windowReleaseDelay)
                } catch {
                    return
                }

                // A window that lands after the sleep ended but before this resumes still cancels it.
                guard !Task.isCancelled, let self, self.visibleWindow == nil else { return }

                self.windowRelease = nil
                engine.releaseWindows()
            }
            return
        }

        await enqueue { [engine] in
            _ = try engine.requestWindow(
                window,
                draft     : draft,
                before    : self.isShowingBefore,
                histogram : false
            )
        }
    }

    /// dismissError acknowledges the error on screen; the next one queued, if any, takes its place.
    func dismissError() {
        guard !errorMessages.isEmpty else { return }

        errorMessages.removeFirst()
    }

    func dismissRenderError() {
        renderErrorMessage = nil
    }

    // MARK: - Chain

    /// enqueue appends `step` to the chain and waits for it. The task is created before the first
    /// suspension, so steps run in the order the callers reached this point. It returns whether
    /// the step finished without throwing.
    @discardableResult
    private func enqueue(_ step: @escaping @MainActor () async throws -> Void) async -> Bool {
        let previous = pending

        let task = Task {
            _ = await previous?.value

            do {
                try await step()
                return true
            } catch EngineError.suspended {
                // Expected when the app resigns active mid-drag: refreshPreview redraws on resume.
            } catch let error as EngineError {
                report(error.message)
            } catch {
                report(error.localizedDescription)
            }

            return false
        }

        pending = task
        return await task.value
    }

    // MARK: - Helpers

    /// report queues an error for the alert, unless the same message is already waiting.
    private func report(_ message: String) {
        if !errorMessages.contains(message) {
            errorMessages.append(message)
        }
    }

    /// show handles a failed render the engine reported. A window is ignored: the preview under
    /// it is still on screen, and the next window asks again. A preview that a newer request
    /// already replaced says nothing about what is shown now.
    private func show(_ failure: FrameFailure) {
        guard !failure.isWindow, failure.generation >= newestPreviewGeneration else { return }

        renderErrorMessage = failure.message
    }

    /// show takes a published preview's histogram unless it is the previous photo's or older than
    /// the one shown (the main actor may run two hops from the engine thread out of order).
    private func show(_ histogram: FrameHistogram) {
        guard histogram.generation > max(histogramFloor, self.histogram?.generation ?? 0) else { return }

        self.histogram = histogram
    }

    /// requestPreview asks for a whole-frame preview and clears the render banner: the new request
    /// reports its own failure if it has one.
    private func requestPreview(draft: Bool) throws(EngineError) -> UInt64 {
        let generation = try engine.requestPreview(
            maxPixels : previewPixels,
            draft     : draft,
            before    : isShowingBefore
        )

        newestPreviewGeneration = generation
        renderErrorMessage      = nil

        return generation
    }

    /// commitPendingNudge commits a stepper burst now instead of when its timer fires. The
    /// `endDrag` link enters the chain before this returns, ahead of whatever the caller queues.
    /// Every action that reaches the engine (a new edit, open, Reset All, a white balance reset,
    /// undo and redo) calls it first, so the burst always ends as its own edit.
    private func commitPendingNudge() {
        guard let pending = nudgeSettle else { return }

        pending.timer.cancel()
        nudgeSettle = nil

        Task.immediate { await self.endDrag(pending.kind) }
    }

    /// requestFrames asks for what the canvas shows after an edit: the preview, and while the canvas
    /// is zoomed past it, the window drawn over it. A draft is the window alone, because that is
    /// what the user looks at and one render per drag step keeps the drag fluid; once per
    /// `histogramInterval` it also carries the whole photo's histogram. A full render brings the
    /// preview (and its histogram) up to date first, then the window (which never replaces the
    /// pending preview, see `requestWindow`). Called from inside a chain link only.
    private func requestFrames(draft: Bool) throws(EngineError) {
        guard let window = visibleWindow else {
            _ = try requestPreview(draft: draft)
            return
        }

        // The preview goes first: the engine renders it before the window it is drawn under.
        if !draft {
            _ = try requestPreview(draft: false)
        }

        _ = try engine.requestWindow(
            window,
            draft     : draft,
            before    : isShowingBefore,
            histogram : draft && isWindowHistogramDue()
        )
    }

    /// isWindowHistogramDue says whether this drag step's window also asks for the histogram, and
    /// records the time when it does.
    private func isWindowHistogramDue() -> Bool {
        let instant = now()

        if let last = lastWindowHistogram, instant - last < histogramInterval {
            return false
        }

        lastWindowHistogram = instant
        return true
    }

    /// current is the snapshot of what the engine holds now, as far as the model has sent or read.
    private var current: EditSnapshot {
        EditSnapshot(values: sent, isWhiteBalanceAsShot: sentAsShot)
    }

    /// noteSet records that a `set` of `kind` landed. The engine's `develop.set` of temperature or
    /// tint always switches the white balance mode to Custom, so no query is needed.
    private func noteSet(of kind: DevelopAdjustmentKind) {
        if kind.isWhiteBalance {
            sentAsShot = false
        }
    }

    /// commitEdit records the snapshot from before an edit once the engine has its result, and only
    /// if something changed. A new edit clears redo and the stack keeps `maxUndoSteps`. Called from
    /// inside a chain link only.
    private func commitEdit() {
        guard current != committed else { return }

        undoStack.append(committed)
        undoStack = Array(undoStack.suffix(Self.maxUndoSteps))
        redoStack = []
        committed = current
    }

    /// step applies the newest snapshot of `source` and, only once the engine has it, moves it to
    /// `target` as the state it left. A failure leaves both stacks as they were: the values are
    /// reread from the engine and the error is shown, and a retry finishes what was left undone.
    private func step(
        from source: ReferenceWritableKeyPath<EditorViewModel, [EditSnapshot]>,
        to target  : ReferenceWritableKeyPath<EditorViewModel, [EditSnapshot]>
    ) async {
        commitPendingNudge()

        await enqueue { [engine] in
            guard let snapshot = self[keyPath: source].last else { return }

            // The burst's commit and every earlier drag link have run: what is left wanted is stale.
            self.wanted = [:]

            let before = self.current

            do throws(EngineError) {
                try await self.apply(snapshot, using: engine)
            } catch {
                try? await self.reloadValues()
                self.values    = self.sent
                self.committed = self.current
                throw error
            }

            self[keyPath: source].removeLast()
            self[keyPath: target].append(before)

            try self.requestFrames(draft: false)
            try await self.reloadEdited()
        }
    }

    /// apply sends `set` only for the controls that differ from what the engine has. A snapshot at
    /// As Shot goes back with `develop.wb`, which restores the mode and the as-shot numbers; one at
    /// Custom sets temperature even when equal while the engine is As Shot, since the set flips the
    /// mode. It updates `sent` after each call, so a failure leaves it true. Called from a chain link.
    private func apply(
        _ snapshot  : EditSnapshot,
        using engine: Engine
    ) async throws(EngineError) {
        let target = snapshot.values

        if snapshot.isWhiteBalanceAsShot {
            let numbersDiffer = [DevelopAdjustmentKind.temperature, .tint].contains { sent[$0] != target[$0] }

            if !sentAsShot || numbersDiffer {
                try await engine.resetWhiteBalance()

                sent[.temperature] = target[.temperature]
                sent[.tint]        = target[.tint]
                sentAsShot         = true
            }
        }

        for kind in DevelopAdjustmentKind.allCases {
            guard let value = target[kind] else { continue }
            guard !(kind.isWhiteBalance && snapshot.isWhiteBalanceAsShot) else { continue }

            let flipsMode = kind == .temperature && sentAsShot

            guard sent[kind] != value || flipsMode else { continue }

            try await engine.set(kind, to: value)
            sent[kind] = value
            noteSet(of: kind)
        }

        values    = sent
        committed = current
    }

    /// reloadValues reads the engine's values, white balance mode and edited flag after a change
    /// the model did not make value by value. Called from inside a chain link only.
    private func reloadValues() async throws(EngineError) {
        let engineValues = try await engine.currentValues()

        values     = Self.defaultValues.merging(engineValues) { _, engineValue in engineValue }
        sent       = values
        sentAsShot = try await engine.isWhiteBalanceAsShot()

        try await reloadEdited()
    }

    /// reloadEdited reads the engine's edited flag of the open photo. Called from inside a chain
    /// link only, after a committed change.
    private func reloadEdited() async throws(EngineError) {
        guard let openPhoto else { return }

        isEdited = try await engine.isEdited(openPhoto)
    }

    /// rollBack puts the engine back on `previous` and deletes the file at `copy`, when given (a
    /// copy this open made for a photo the engine never opened). Its own failures are only logged:
    /// the open's error is the one the user sees.
    private static func rollBack(
        engine          : Engine,
        to previous     : PhotoID?,
        discarding copy : URL?
    ) async {
        let logger = Logger(subsystem: "com.eliorodr2104.unveil", category: "EditorViewModel")

        if let previous {
            do {
                try await engine.select(previous)
            } catch {
                logger.error("Re-selecting the previous photo failed: \(error.message, privacy: .public)")
            }
        }

        if let copy {
            do {
                try FileManager.default.removeItem(at: copy)
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
        defer {
            Signposts.signposter.endInterval("ImportCopy", signpost)
            OpenTimeline.mark("copy-end")
        }

        OpenTimeline.mark("copy-begin")
        return try importer.importCopy(of: pickedURL)
    }

    private static func message(for error: PhotoImportError) -> String {
        switch error {
            case .accessDenied(let url):
                return "Cannot read \(url.lastPathComponent)."

            case .copyFailed(let text):
                return "Could not copy the file: \(text)"
        }
    }
}
