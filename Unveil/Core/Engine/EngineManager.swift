//
//  EngineManager.swift
//  Unveil
//

import CoreGraphics
import Foundation
import Metal
import os
import UnveilEngine

/// EngineManager is the only door to the engine: no other type calls `uv_*`. It owns the session
/// handle from `uv_session_new` and the FrameSink its preview callback fills.
///
/// Commands (`uv_execute`) run one at a time on a private serial queue, because each one blocks
/// until the engine thread answers (up to 30 s) and the caller must never wait on that. The async
/// methods put their work on the queue synchronously, before their first suspension point, so two
/// calls made in a row by the same task reach the engine in that order.
///
/// `requestPreview`, `suspend`, `resume` and `setMemoryBudget` run on the caller's thread instead:
/// the first only flips atomics and posts a message, the others are short by contract
/// (`uv_suspend` waits at most 2 s for a render in flight, the caller decides where it may block).
///
/// `uv_last_error` is per thread and cleared by every `uv_*` call, so each failure reads it on the
/// thread that failed, right after the failing call, before any other `uv_*` call.
///
/// Sendable is asserted, not inferred: the session is an OpaquePointer, which Swift cannot prove
/// Sendable. It is never mutated after init, and every `uv_*` function is documented as callable
/// from any thread; the engine serialises the work on its own thread behind that handle.
// Nonisolated: the queue and the engine thread call into it, and the app default is MainActor.
nonisolated final class EngineManager:
    EngineDriving,
    EngineDiagnosing,
    ImportCatalogReading,
    @unchecked Sendable
{

    let frames: FrameSink

    private let session  : OpaquePointer
    private let maxPixels: Int
    private let queue    = DispatchQueue(label: "com.unveil.engine-manager", qos: .userInitiated)

    /// SessionHandle carries the session pointer to the queue that frees it in deinit.
    /// OpaquePointer is not Sendable; nothing else holds it by then, so the hand-off is safe.
    private struct SessionHandle: @unchecked Sendable {

        let pointer: OpaquePointer
    }

    /// ImportRequest is `library.import`'s params. `onDeleted: restore` brings a trashed photo back
    /// in `restored`, instead of a duplicate whose `existing` id is null.
    private struct ImportRequest: Encodable {

        let paths     : [String]
        let mode      = "add"
        let onDeleted = "restore"
    }

    /// ImportReport is the part of `library.import`'s result that names the photo.
    /// `failed` holds `[path, reason]` pairs for the files the engine could not read.
    private struct ImportReport: Decodable {

        let imported  : [UInt64]
        let restored  : [UInt64]?
        let duplicates: [Duplicate]
        let failed    : [[String]]

        /// Duplicate names the photo that already has the file. `reason` is "path" when that photo
        /// was imported from this very path, "content" when the same bytes live elsewhere.
        struct Duplicate: Decodable {

            let existing: UInt64?
            let reason  : String
        }
    }

    private struct SelectRequest: Encodable {

        let ids   : [UInt64]
        let active: UInt64
    }

    private struct RelinkRequest: Encodable {

        let id  : UInt64
        let path: String
    }

    private struct DevelopMergeRequest: Encodable {

        let settings: DevelopMergeSettings
    }

    /// CatalogQueryRequest is `catalog.query` narrowed to one photo with `filter.only`.
    private struct CatalogQueryRequest: Encodable {

        let filter: Filter
        let limit = 1

        struct Filter: Encodable {

            let only: [UInt64]
        }
    }

    /// CatalogQueryResult is the part of `catalog.query`'s photo summaries the editor reads.
    private struct CatalogQueryResult: Decodable {

        let photos: [Summary]

        struct Summary: Decodable {

            let edited: Bool
        }
    }

    /// CatalogPageRequest is one page of `catalog.query` over the library, or over Recently Deleted.
    private struct CatalogPageRequest: Encodable {

        let filter: Filter
        let offset: Int
        let limit : Int

        struct Filter: Encodable {

            let deleted: Bool
        }
    }

    /// CatalogPageResult is one page of photo summaries, with `total`, the size of the whole answer.
    private struct CatalogPageResult: Decodable {

        let total : Int
        let photos: [Summary]

        struct Summary: Decodable {

            let fileName: String
        }
    }

    /// DevelopReading is the part of `develop.get` the editor reads: the white balance mode.
    private struct DevelopReading: Decodable {

        let wb: WhiteBalance

        struct WhiteBalance: Decodable {

            let mode: String
        }
    }

    private struct DevelopSetRequest: Encodable {

        let control: String
        let value  : Double
    }

    /// ControlReading is one row of `develop.controls`: the control id and its current value, which
    /// is null for nothing the engine can read. The rest of the row (range, label) is ignored.
    private struct ControlReading: Decodable {

        let id   : String
        let value: Double?
    }

    private struct IDsRequest: Encodable {

        let ids: [UInt64]
    }

    private struct IDRequest: Encodable {

        let id: UInt64
    }

    private struct WhiteBalanceRequest: Encodable {

        let mode: String
    }

    /// init(dataDirectory:maxPixels:) opens the engine's library in `dataDirectory`, creating the
    /// folder first (Application Support does not exist on a fresh install), and allocates a FrameSink
    /// whose long edge is `maxPixels`. It blocks until the engine thread has the library open, so the
    /// app makes one at launch. Two managers on the same directory fail: the library is file-locked.
    init(
        dataDirectory: URL = URL.applicationSupportDirectory.appending(path: "Engine", directoryHint: .isDirectory),
        maxPixels    : Int
    ) throws(EngineError) {
        // The header and the linked library must agree on the C ABI before anything else is called.
        guard uv_abi_version() == UInt32(UV_ABI_VERSION) else { throw .engine("ABI mismatch") }
        guard maxPixels > 0 else { throw .invalidArgument("maxPixels must be positive, got \(maxPixels)") }
        guard let device = MTLCreateSystemDefaultDevice() else { throw .engine("no Metal device") }

        // Out of the iCloud backup, like the Imports copies its records point at: the edits it holds
        // are therefore not backed up either (a v0 trade-off, by ruling).
        do {
            var directory = dataDirectory
            var noBackup  = URLResourceValues()
            noBackup.isExcludedFromBackup = true

            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try directory.setResourceValues(noBackup)
        } catch {
            throw .io("cannot create \(dataDirectory.path(percentEncoded: false)): \(error.localizedDescription)")
        }

        // A budget of 0 keeps the engine default until MemoryBudgetMonitor sets one.
        let session = dataDirectory.withUnsafeFileSystemRepresentation { uv_session_new($0, 0) }
        guard let session else { throw .engine(Self.lastError(or: "the engine session did not open")) }

        self.session   = session
        self.maxPixels = maxPixels
        self.frames    = FrameSink(device: device, maxPixels: maxPixels)
    }

    /// deinit frees the session on a utility queue: `uv_session_free` waits up to 10 s for a render
    /// in flight, and the last release may come from the main thread. The sink is the callback's
    /// `ctx`, so it is kept alive until the free has returned and no frame can arrive.
    deinit {
        let handle = SessionHandle(pointer: session)
        let frames = frames

        DispatchQueue.global(qos: .utility).async {
            withExtendedLifetime(frames) {
                uv_session_free(handle.pointer)
            }
        }
    }

    /// openPhoto imports the file into the library and makes it the active photo.
    ///
    /// The id comes from `imported`, else `restored` (it was in the trash), else `duplicates` (the
    /// path or the bytes are already in the library), so reopening a photo works like a first open.
    /// An existing record is relinked to `fileURL` before it is selected: iOS gives the app a new
    /// data container when it is reinstalled, so the path the library stored may no longer exist,
    /// and a render of it would never deliver a frame. The copy just imported is the live file.
    /// A "path" duplicate is not relinked: the record already points at `fileURL`, and a relink
    /// would only re-hash the whole file (about 286 ms on a 46 MP NEF).
    /// A file the engine cannot read still returns UV_OK, with the reason in `failed`: that is
    /// how an unreadable file becomes an error here.
    func openPhoto(at fileURL: URL) async throws(EngineError) -> PhotoID {
        try await onQueue { () throws(EngineError) -> PhotoID in
            let request = ImportRequest(paths: [fileURL.path(percentEncoded: false)])
            let report  = try self.decode(ImportReport.self, from: self.execute("library.import", request))

            let existing = report.restored?.first ?? report.duplicates.first?.existing

            guard let id = report.imported.first ?? existing
            else {
                if let failure = report.failed.first {
                    throw .engine(failure.dropFirst().first ?? "the engine could not import the file")
                }
                throw .engine("nothing imported")
            }

            if report.imported.isEmpty, report.duplicates.first?.reason != "path" {
                let relink = RelinkRequest(id: id, path: fileURL.path(percentEncoded: false))
                _ = try self.execute("photo.relink", relink)
            }

            _ = try self.execute("library.select", SelectRequest(ids: [id], active: id))
            return PhotoID(rawValue: id)
        }
    }

    /// select makes `photo` the active photo. The editor uses it to put the engine back on the
    /// photo it still shows when an open fails halfway.
    func select(_ photo: PhotoID) async throws(EngineError) {
        try await onQueue { () throws(EngineError) in
            _ = try self.execute("library.select", SelectRequest(ids: [photo.rawValue], active: photo.rawValue))
        }
    }

    /// set sends one develop control. The engine clamps values outside the control's range, but JSON
    /// has no NaN or infinity, so a non-finite value is refused here.
    func set(_ adjustment: DevelopAdjustmentKind, to value: Double) async throws(EngineError) {
        guard value.isFinite else { throw .invalidArgument("\(adjustment.rawValue): \(value) is not finite") }

        try await onQueue { () throws(EngineError) in
            _ = try self.execute("develop.set", DevelopSetRequest(control: adjustment.rawValue, value: value))
        }
    }

    /// currentValues reads the active photo's controls with `develop.controls` and keeps the ones
    /// the editor shows. The command lists every control with its id, which is also our raw value,
    /// so no mapping table is needed.
    func currentValues() async throws(EngineError) -> [DevelopAdjustmentKind: Double] {
        try await onQueue { () throws(EngineError) -> [DevelopAdjustmentKind: Double] in
            let result   = try self.execute("develop.controls", [String: String]())
            let readings = try self.decode([ControlReading].self, from: result)
            var values   = [DevelopAdjustmentKind: Double]()

            for reading in readings {
                if let kind = DevelopAdjustmentKind(rawValue: reading.id), let value = reading.value {
                    values[kind] = value
                }
            }

            return values
        }
    }

    /// isEdited reads the `edited` flag of the photo's summary (`Photo::is_edited`). `catalog.query`
    /// is the only command that reports it; `photo.inspect`'s `edited` is the last edit's time.
    func isEdited(_ photo: PhotoID) async throws(EngineError) -> Bool {
        try await onQueue { () throws(EngineError) -> Bool in
            let request = CatalogQueryRequest(filter: .init(only: [photo.rawValue]))
            let json    = try self.execute("catalog.query", request)

            guard let summary = try self.decode(CatalogQueryResult.self, from: json).photos.first
            else {
                throw .engine("photo \(photo.rawValue) is not in the library")
            }

            return summary.edited
        }
    }

    /// referencedFileNames lists the file name of every photo the catalog holds, the trashed ones too
    /// (a restore needs the file), from `catalog.query` summaries a page at a time. The engine keeps
    /// `fileName` equal to the stored file's name, at import and on relink.
    ///
    /// It is one queue block, so no open lands between the pages. An answer is complete only when
    /// the pages add up to the `total` the engine reported; anything else throws, like a failure.
    /// Browsed ("local") photos are not in the answer: the app never browses a folder.
    func referencedFileNames() async throws(EngineError) -> Set<String> {
        try await onQueue { () throws(EngineError) -> Set<String> in
            let pageSize = 500
            var names    = Set<String>()

            for isDeleted in [false, true] {
                var listed = 0
                var total  = 0

                repeat {
                    let request = CatalogPageRequest(
                        filter : .init(deleted: isDeleted),
                        offset : listed,
                        limit  : pageSize
                    )
                    let json = try self.execute("catalog.query", request)
                    let page = try self.decode(CatalogPageResult.self, from: json)

                    total   = page.total
                    listed += page.photos.count
                    names.formUnion(page.photos.map(\.fileName))

                    guard !page.photos.isEmpty || listed >= total else {
                        throw .engine("catalog.query stopped at \(listed) of \(total) photos")
                    }
                } while listed < total

                guard listed == total else {
                    throw .engine("catalog.query listed \(listed) photos, total \(total)")
                }
            }

            return names
        }
    }

    func isWhiteBalanceAsShot() async throws(EngineError) -> Bool {
        try await onQueue { () throws(EngineError) -> Bool in
            let result = try self.execute("develop.get", [String: String]())
            return try self.decode(DevelopReading.self, from: result).wb.mode == "asShot"
        }
    }

    func photoInfo(of photo: PhotoID) async throws(EngineError) -> PhotoInfo {
        try await onQueue { () throws(EngineError) -> PhotoInfo in
            let result = try self.execute("photo.inspect", IDRequest(id: photo.rawValue))
            return try self.decode(PhotoInfo.self, from: result)
        }
    }

    /// resetDevelop names the photo in `ids`: without it the engine resets the whole selection.
    /// Both commands run in one queue block, so nothing lands between them.
    func resetDevelop(
        _ photo            : PhotoID,
        restoring settings : DevelopMergeSettings
    ) async throws(EngineError) {
        try await onQueue { () throws(EngineError) in
            _ = try self.execute("develop.reset", IDsRequest(ids: [photo.rawValue]))

            guard !settings.isEmpty else { return }

            _ = try self.execute("develop.merge", DevelopMergeRequest(settings: settings))
        }
    }

    func resetWhiteBalance() async throws(EngineError) {
        try await onQueue { () throws(EngineError) in
            _ = try self.execute("develop.wb", WhiteBalanceRequest(mode: "asShot"))
        }
    }

    /// requestPreview asks for a render of the active photo whose long edge fits `maxPixels`, which
    /// must fit the sink as well, and returns its generation at once. The frame lands in `frames` on
    /// the engine thread; it may never come if a newer request overtakes it. `before` asks for the
    /// photo's unedited look.
    func requestPreview(maxPixels: Int, draft: Bool, before: Bool) throws(EngineError) -> UInt64 {
        guard maxPixels > 0, maxPixels <= self.maxPixels else {
            throw .invalidArgument("maxPixels must be in 1...\(self.maxPixels), got \(maxPixels)")
        }

        let generation = FrameTimingProbe.shared.measure("RequestPreview") {
            uv_request_preview(
                session,
                UInt32(maxPixels),
                draft,
                before,
                Self.receiveFrame,
                Unmanaged.passUnretained(frames).toOpaque()
            )
        }

        return try Self.requested(generation, isDraft: draft, isWindow: false)
    }

    /// requestWindow asks for one window of the zoomed frame (see FrameWindow) and returns its
    /// generation at once. The window must fit the sink; the frame lands in `frames` as its window.
    /// The frame is asked for by its long edge (a square of `frameLong`, see FrameWindow). With
    /// `histogram` its frame also carries the whole photo's histogram (ABI 6).
    func requestWindow(
        _ window  : FrameWindow,
        draft     : Bool,
        before    : Bool,
        histogram : Bool
    ) throws(EngineError) -> UInt64 {
        guard window.width  > 0, window.width  <= maxPixels,
              window.height > 0, window.height <= maxPixels,
              window.frameWidth > 0, window.frameHeight > 0
        else {
            throw .invalidArgument("the window must fit 1...\(maxPixels) on each side, got \(window)")
        }

        let generation = FrameTimingProbe.shared.measure("RequestWindow") {
            uv_request_window(
                session,
                UInt32(clamping: window.frameLong),
                UInt32(clamping: window.frameLong),
                UInt32(clamping: window.x),
                UInt32(clamping: window.y),
                UInt32(window.width),
                UInt32(window.height),
                draft,
                before,
                histogram,
                Self.receiveFrame,
                Unmanaged.passUnretained(frames).toOpaque()
            )
        }

        return try Self.requested(generation, isDraft: draft, isWindow: true)
    }

    /// receiveFrame is the C callback of both requests, on the engine thread. It cannot capture: it
    /// reaches the sink through `ctx`, unretained because the manager owns the sink and deinit keeps
    /// it alive until the session is freed. Internal so the C delivery contract can be tested
    /// without creating a session or invoking a render.
    static let receiveFrame: uv_frame_cb = { context, frame in
        guard let context, let frame else { return }

        let info = frame.pointee
        let sink = Unmanaged<FrameSink>.fromOpaque(context).takeUnretainedValue()

        // A failed render has no pixels: its message goes to the sink, and the frames stay.
        guard info.status == UV_OK.rawValue, let rgba = info.rgba else {
            let message = info.error.map { String(cString: $0) } ?? "the render failed"

            Signposts.frameFailed(generation: info.generation)
            sink.receiveFailure(FrameFailure(
                generation : info.generation,
                isWindow   : info.window,
                message    : message
            ))
            return
        }

        let window = info.window ? FrameWindow(
            frameWidth  : Int(info.frame_width),
            frameHeight : Int(info.frame_height),
            x           : Int(info.x),
            y           : Int(info.y),
            width       : Int(info.display_width),
            height      : Int(info.display_height)
        ) : nil

        OpenTimeline.mark("frame-delivered gen \(info.generation) \(info.window ? "window" : "fit") draft \(info.draft)")

        sink.receive(
            rgba          : UnsafeRawPointer(rgba),
            width         : Int(info.width),
            height        : Int(info.height),
            stride        : Int(info.stride),
            generation    : info.generation,
            isDraft       : info.draft,
            window        : window,
            replacesWindow: info.fallback,
            nativeWidth   : Int(info.native_width),
            nativeHeight  : Int(info.native_height),
            histogram     : info.histogram.map { EngineManager.histogram(of: $0.pointee, frame: info) },
            displayWidth  : Int(info.display_width),
            displayHeight : Int(info.display_height)
        )
    }

    /// histogram(of:frame:) copies a frame's UVHistogram, valid only during the callback, into
    /// arrays. Its fixed C arrays reach Swift as tuples of UInt32.
    private static func histogram(of raw: UVHistogram, frame: UVFrame) -> FrameHistogram {
        // A homogeneous tuple is laid out as a C array, its memory already bound to UInt32.
        func bins<Tuple>(_ tuple: Tuple) -> [UInt32] {
            withUnsafeBytes(of: tuple) { Array($0.assumingMemoryBound(to: UInt32.self)) }
        }

        return FrameHistogram(
            red         : bins(raw.r),
            green       : bins(raw.g),
            blue        : bins(raw.b),
            luma        : bins(raw.luma),
            total       : Int(raw.total),
            clippedLow  : bins(raw.clipped_low).map(Int.init),
            clippedHigh : bins(raw.clipped_high).map(Int.init),
            generation  : frame.generation,
            isDraft     : frame.draft
        )
    }

    /// releaseWindows frees what zoom windows keep resident in the engine (`uv_release_windows`):
    /// it only posts a message, so it never waits on the engine.
    func releaseWindows() {
        FrameTimingProbe.shared.measure("ReleaseWindows") { uv_release_windows(session) }
    }

    /// requested returns a request's generation and begins its `SliderToFrame`, or throws the
    /// refusal that a generation of 0 means. It must run right after the request, on its thread.
    private static func requested(
        _ generation: UInt64,
        isDraft     : Bool,
        isWindow    : Bool
    ) throws(EngineError) -> UInt64 {
        guard generation == 0 else {
            Signposts.previewRequested(generation: generation, isDraft: isDraft, isWindow: isWindow)
            return generation
        }

        // ponytail: the requests have no status code, so "suspended" is told apart by its
        // documented message; a status out-parameter in uv.h would replace the string match.
        let message = lastError(or: "the preview was refused")
        throw message == "the session is suspended" ? .suspended(message) : .engine(message)
    }

    /// diagnostic runs one engine command and returns its JSON result untouched. It is the single
    /// door for the diagnostics (`app.gpu`, `develop.reset`, `library.memory`), on the command queue
    /// like every other command. Never send `app.gpu` anything but `{}`: it would change the preference.
    func diagnostic(_ command: String, params: some Encodable & Sendable) async throws(EngineError) -> String {
        try await onQueue { () throws(EngineError) -> String in
            let result = try self.execute(command, params)
            return String(decoding: result, as: UTF8.self)
        }
    }

    /// recordState queues a read of `app.gpu` on the command queue and appends it to `engine.jsonl`.
    /// It returns at once, so main never waits on it, and the read lands behind any command in flight.
    func recordState(event: String) {
        queue.async {
            let gpu: String

            do throws(EngineError) {
                gpu = String(decoding: try self.execute("app.gpu", [String: String]()), as: UTF8.self)
            } catch {
                gpu = EngineStateLog.errorJSON(error.message)
            }

            EngineStateLog.append(
                event         : event,
                gpu           : gpu,
                possibleTears : nil
            )
        }
    }

    /// suspend stops GPU work for the background and blocks until the render in flight is done,
    /// at most 2 s. On `.timeout` the session is suspended anyway, so `resume()` is still needed.
    ///
    /// Whatever the outcome, it records the engine state afterwards: `resume()` clears the engine's
    /// last GPU fallback, so a fallback during the background is only visible between the two.
    func suspend() throws(EngineError) {
        defer { recordState(event: "suspend") }

        try Self.check(uv_suspend(session))
    }

    func resume() throws(EngineError) {
        try Self.check(uv_resume(session))
    }

    func setMemoryBudget(bytes: UInt64) {
        uv_set_memory_budget(session, bytes)
    }

    /// onQueue runs `work` on the command queue and suspends until it is done. The work is queued
    /// synchronously, inside the continuation's body, which keeps the callers' order (see the type doc).
    private func onQueue<Value: Sendable>(
        _ work: @escaping @Sendable () throws(EngineError) -> Value
    ) async throws(EngineError) -> Value {
        let result: Result<Value, EngineError> = await withCheckedContinuation { continuation in
            queue.async {
                continuation.resume(returning: Result(catching: work))
            }
        }

        return try result.get()
    }

    /// execute runs one engine command and returns its JSON result. Called on the command queue only.
    /// Each call is a `Command` signpost named after the command, so a trace shows where `open` goes.
    private func execute(_ command: String, _ params: some Encodable) throws(EngineError) -> Data {
        let signpost = Signposts.signposter.beginInterval(
            "Command",
            id: Signposts.signposter.makeSignpostID(),
            "\(command, privacy: .public)"
        )
        OpenTimeline.mark("cmd-begin \(command)")
        defer {
            Signposts.signposter.endInterval("Command", signpost)
            OpenTimeline.mark("cmd-end \(command)")
        }

        let json: Data
        do {
            json = try JSONEncoder().encode(params)
        } catch {
            throw .invalidArgument("\(command): \(error.localizedDescription)")
        }

        var output: UnsafeMutablePointer<CChar>?
        let status = String(decoding: json, as: UTF8.self).withCString { params in
            uv_execute(session, command, params, &output)
        }

        try Self.check(status)
        guard let output else { throw .engine("\(command) returned no result") }

        let result = Data(String(cString: output).utf8)
        uv_string_free(output)
        return result
    }

    private func decode<Value: Decodable>(_ type: Value.Type, from json: Data) throws(EngineError) -> Value {
        do {
            return try JSONDecoder().decode(type, from: json)
        } catch {
            throw .engine("unexpected engine result: \(error.localizedDescription)")
        }
    }

    /// check turns a UVStatus into an error. It must run right after the call that returned
    /// `status`, on the same thread, or `uv_last_error` no longer describes it.
    private static func check(_ status: Int32) throws(EngineError) {
        guard status != UV_OK.rawValue else { return }
        throw EngineError(status: status, message: lastError(or: "status \(status)"))
    }

    private static func lastError(or fallback: String) -> String {
        guard let message = uv_last_error() else { return fallback }
        return String(cString: message)
    }
}
