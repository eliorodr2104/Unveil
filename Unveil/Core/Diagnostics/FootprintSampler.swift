//
//  FootprintSampler.swift
//  Unveil
//

import Foundation
import os

/// FootprintSampler writes the app's memory and thermal state every 2 s to
/// `Documents/measurements/<ISO date>.csv`, one row per sample:
/// `time,elapsed_s,phys_footprint_bytes,available_bytes,thermal_state`.
///
/// `phys_footprint` (from `task_info` with `TASK_VM_INFO`) is the number jetsam judges the app by,
/// so it is the honest peak to report; `os_proc_available_memory()` is how far the app is from that
/// limit (0 on the simulator). Each row is also printed, because `devicectl --console` shows stdout.
///
/// It runs only while StressSweep runs, which only a menu tap or a launch argument starts. The loop
/// is a detached utility task that sleeps between samples, so main never waits on the disk.
final class FootprintSampler {

    nonisolated static let sampleInterval = Duration.seconds(2)
    nonisolated static let header         =
        "time,elapsed_s,phys_footprint_bytes,available_bytes,thermal_state"

    let fileURL: URL

    private var task: Task<Void, Never>?

    init(
        directory: URL = URL.documentsDirectory.appending(path: "measurements", directoryHint: .isDirectory),
        startDate: Date = .now
    ) {
        let name = startDate.formatted(.iso8601.timeSeparator(.omitted))
        self.fileURL = directory.appending(path: "\(name).csv")
    }

    func start() {
        guard task == nil else { return }

        let fileURL = fileURL
        task = Task.detached(priority: .utility) {
            await Self.sample(into: fileURL)
        }
    }

    /// stop cancels the loop and waits for it, so the file is complete when it returns. The loop
    /// writes one last sample on the way out, so a peak or a thermal change just before stop is kept.
    func stop() async {
        task?.cancel()
        await task?.value
        task = nil
    }

    /// row formats one sample. Elapsed seconds keep one decimal (the interval is 2 s), with a dot
    /// whatever the device locale; a footprint the kernel did not report is an empty field, not 0.
    nonisolated static func row(
        time      : Date,
        elapsed   : Double,
        footprint : UInt64?,
        available : UInt64,
        thermal   : ProcessInfo.ThermalState
    ) -> String {
        let seconds   = String(format: "%.1f", elapsed)
        let footprint = footprint.map(String.init) ?? ""
        return "\(time.formatted(.iso8601)),\(seconds),\(footprint),\(available),\(name(of: thermal))"
    }

    nonisolated static func name(of thermal: ProcessInfo.ThermalState) -> String {
        switch thermal {
            case .nominal:  return "nominal"
            case .fair:     return "fair"
            case .serious:  return "serious"
            case .critical: return "critical"
            @unknown default: return "unknown"
        }
    }

    /// physicalFootprint reads `phys_footprint` from the kernel, or nil if `task_info` fails.
    nonisolated static func physicalFootprint() -> UInt64? {
        var info  = task_vm_info_data_t()
        let words = MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size
        var count = mach_msg_type_number_t(words)

        // task_info fills `count` words of the struct it was given; `count` is its size in words.
        let status = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { words in
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), words, &count)
            }
        }

        return status == KERN_SUCCESS ? info.phys_footprint : nil
    }

    @concurrent
    private nonisolated static func sample(into fileURL: URL) async {
        let logger = Logger(subsystem: "com.eliorodr2104.unveil", category: "FootprintSampler")
        let handle: FileHandle

        do {
            try FileManager.default.createDirectory(
                at                          : fileURL.deletingLastPathComponent(),
                withIntermediateDirectories : true
            )
            try Data((header + "\n").utf8).write(to: fileURL)
            handle = try FileHandle(forWritingTo: fileURL)
            try handle.seekToEnd()
        } catch {
            logger.error("Cannot create the CSV: \(error.localizedDescription, privacy: .public)")
            return
        }

        defer { try? handle.close() }

        print("footprint csv \(fileURL.path(percentEncoded: false))")
        let clock = ContinuousClock()
        let start = clock.now
        var count = 0

        // The cancellation check sits after the write, so the sleep cut short by stop() still
        // ends in one final sample.
        while true {
            let line = row(
                time      : .now,
                elapsed   : (clock.now - start) / .seconds(1),
                footprint : physicalFootprint(),
                available : UInt64(os_proc_available_memory()),
                thermal   : ProcessInfo.processInfo.thermalState
            )

            print("footprint \(line)")

            do {
                try handle.write(contentsOf: Data((line + "\n").utf8))
            } catch {
                logger.error("Cannot append a sample: \(error.localizedDescription, privacy: .public)")
            }

            if Task.isCancelled { break }

            // Sleeping to a deadline, not for an interval, keeps the samples from drifting late.
            count += 1
            try? await clock.sleep(until: start + sampleInterval * count)
        }
    }
}
