//
//  DecodeMemorySampler.swift
//  Unveil
//

import Foundation
import Metal

/// DecodeMemorySampler bounds the 100 ms trace for `-UnveilDecodeMemory`.
/// No region walk or file I/O runs in the loop.
/// Rows are printed after the measurement; the kernel lifetime peak survives gaps between samples.
final class DecodeMemorySampler {

    private var task: Task<[[String: UInt64]], Never>?

    /// start samples until cancellation or the two-minute diagnostic limit.
    func start(device: any MTLDevice) {
        guard task == nil else { return }

        task = Task.detached(priority: .utility) {
            let clock = ContinuousClock()
            let start = clock.now
            var rows  = [[String: UInt64]]()

            rows.reserveCapacity(1200)

            for index in 0 ..< 1200 {
                var row               = MemoryMapReport.footprint()
                row["timeNs"]         = ProcessCostProbe.uptimeNs()
                row["metalAllocated"] = UInt64(device.currentAllocatedSize)
                rows.append(row)

                if Task.isCancelled { break }

                try? await clock.sleep(until: start + .milliseconds(100) * (index + 1))
            }

            return rows
        }
    }

    /// stop waits for the sampler before emitting the collected rows outside the measured loop.
    func stop() async {
        guard let task else { return }

        task.cancel()

        for row in await task.value {
            EditorLaunchRunner.emit("DM", row)
        }

        self.task = nil
    }
}
