//
//  EngineStateLog.swift
//  Unveil
//

import Foundation
import os
import Synchronization

// Nonisolated: the engine's command queue and the main actor both append, and the app default is MainActor.
/// EngineStateLog records what the engine reports about its GPU backend, one JSON line per event
/// in `Documents/diagnostics/engine.jsonl`: `{time, event, gpu, possibleTears?}`. `gpu` is the
/// `app.gpu` answer as the engine gave it.
///
/// The same line also goes to `os.Logger` (public) and to stdout, because `devicectl --console`
/// shows stdout and the file needs a copy of the device to be read. The file is appended under one
/// Mutex: lines come from the command queue (launch, suspend) and from the main actor (the menu),
/// and two interleaved writes would break the JSON Lines format.
///
/// Why the suspend event matters: `uv_resume` clears the engine's last GPU fallback, so a fallback
/// that happened while the app was in the background is only visible between suspend and resume.
///
/// Every suspend writes a line, so the file is rotated rather than left to grow for the life of the
/// install: past `rotationBytes` it becomes `engine.jsonl.1` (replacing the older one) and a new
/// file starts. At most two files, about 512 KB, and the newest lines are always kept.
nonisolated enum EngineStateLog {

    private static let fileLock      = Mutex(())
    private static let logger        = Logger(subsystem: "com.eliorodr2104.unveil", category: "EngineState")
    private static let rotationBytes = 256 * 1024

    /// fileURL is `Documents/diagnostics/engine.jsonl`, reachable from the Files app.
    static var fileURL: URL {
        URL.documentsDirectory.appending(path: "diagnostics/engine.jsonl")
    }

    /// errorJSON wraps a failure in the shape of a `gpu` value, so a failed read is still a line.
    static func errorJSON(_ message: String) -> String {
        let object = ["error": message]
        let data   = (try? JSONSerialization.data(withJSONObject: object)) ?? Data()
        return String(decoding: data, as: UTF8.self)
    }

    /// append writes one line. `gpu` is the JSON text of `app.gpu` (or errorJSON); if it does not
    /// parse it is kept as a string, so no line is ever lost to a malformed answer.
    static func append(
        event         : String,
        gpu           : String,
        possibleTears : Int?
    ) {
        let gpuValue = (try? JSONSerialization.jsonObject(with: Data(gpu.utf8))) ?? gpu

        var line: [String: Any] = [
            "time"  : Date.now.formatted(.iso8601),
            "event" : event,
            "gpu"   : gpuValue,
        ]

        if let possibleTears {
            line["possibleTears"] = possibleTears
        }

        let options: JSONSerialization.WritingOptions = [.sortedKeys, .withoutEscapingSlashes]

        guard let data = try? JSONSerialization.data(withJSONObject: line, options: options) else {
            logger.error("Engine state line for \(event, privacy: .public) is not valid JSON")
            return
        }

        let text = String(decoding: data, as: UTF8.self)
        logger.info("engine-state \(text, privacy: .public)")
        print("engine-state \(text)")

        fileLock.withLock { _ in
            do {
                try appendToFile(data + Data("\n".utf8))
            } catch {
                logger.error("Cannot append to engine.jsonl: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    private static func appendToFile(_ bytes: Data) throws {
        let fileManager = FileManager.default
        let url         = fileURL

        try fileManager.createDirectory(
            at                          : url.deletingLastPathComponent(),
            withIntermediateDirectories : true
        )

        let size = (try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0

        if size > rotationBytes {
            let previous = url.appendingPathExtension("1")

            try? fileManager.removeItem(at: previous)   // Absent before the first rotation.
            try fileManager.moveItem(at: url, to: previous)
        }

        if !fileManager.fileExists(atPath: url.path(percentEncoded: false)) {
            try Data().write(to: url)
        }

        let handle = try FileHandle(forWritingTo: url)
        defer { try? handle.close() }

        try handle.seekToEnd()
        try handle.write(contentsOf: bytes)
    }
}
