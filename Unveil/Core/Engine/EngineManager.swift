//
//  EngineManager.swift
//  Unveil
//

import Foundation
import Metal
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
nonisolated final class EngineManager: EngineDriving, @unchecked Sendable {

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

        struct Duplicate: Decodable {

            let existing: UInt64?
        }
    }

    private struct SelectRequest: Encodable {

        let ids   : [UInt64]
        let active: UInt64
    }

    private struct DevelopSetRequest: Encodable {

        let control: String
        let value  : Double
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

        do {
            try FileManager.default.createDirectory(at: dataDirectory, withIntermediateDirectories: true)
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
    /// A file the engine cannot read still returns UV_OK, with the reason in `failed`: that is
    /// how an unreadable file becomes an error here.
    func openPhoto(at fileURL: URL) async throws(EngineError) -> PhotoID {
        try await onQueue { () throws(EngineError) -> PhotoID in
            let request = ImportRequest(paths: [fileURL.path(percentEncoded: false)])
            let report  = try self.decode(ImportReport.self, from: self.execute("library.import", request))

            guard let id = report.imported.first ?? report.restored?.first ?? report.duplicates.first?.existing
            else {
                if let failure = report.failed.first {
                    throw .engine(failure.dropFirst().first ?? "the engine could not import the file")
                }
                throw .engine("nothing imported")
            }

            _ = try self.execute("library.select", SelectRequest(ids: [id], active: id))
            return PhotoID(rawValue: id)
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

    /// requestPreview asks for a render of the active photo whose long edge fits `maxPixels`, which
    /// must fit the sink as well, and returns its generation at once. The frame lands in `frames` on
    /// the engine thread; it may never come if a newer request overtakes it.
    func requestPreview(maxPixels: Int, draft: Bool) throws(EngineError) -> UInt64 {
        guard maxPixels > 0, maxPixels <= self.maxPixels else {
            throw .invalidArgument("maxPixels must be in 1...\(self.maxPixels), got \(maxPixels)")
        }

        // The callback cannot capture: it reaches the sink through `ctx`, unretained because
        // the manager owns the sink and deinit keeps it alive until the session is freed.
        let generation = uv_request_preview(
            session,
            UInt32(maxPixels),
            draft,
            { context, rgba, width, height, stride, generation, isDraft in
                guard let context, let rgba else { return }

                Unmanaged<FrameSink>.fromOpaque(context).takeUnretainedValue().receive(
                    rgba      : UnsafeRawPointer(rgba),
                    width     : Int(width),
                    height    : Int(height),
                    stride    : Int(stride),
                    generation: generation,
                    isDraft   : isDraft
                )
            },
            Unmanaged.passUnretained(frames).toOpaque()
        )

        guard generation == 0 else { return generation }

        // ponytail: uv_request_preview has no status code, so "suspended" is told apart by its
        // documented message; a status out-parameter in uv.h would replace the string match.
        let message = Self.lastError(or: "the preview was refused")
        throw message == "the session is suspended" ? .suspended(message) : .engine(message)
    }

    /// suspend stops GPU work for the background and blocks until the render in flight is done,
    /// at most 2 s. On `.timeout` the session is suspended anyway, so `resume()` is still needed.
    func suspend() throws(EngineError) {
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
    private func execute(_ command: String, _ params: some Encodable) throws(EngineError) -> Data {
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
