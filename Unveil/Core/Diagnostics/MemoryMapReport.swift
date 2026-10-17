//
//  MemoryMapReport.swift
//  Unveil
//

import Darwin
import Foundation
import Metal
import os

/// MemoryMapReport is one moment of the E0 memory map: everything the app can read about its own
/// memory, as one JSON line printed as `MM {...}` (so `devicectl --console` carries it to the Mac).
///
/// - `footprint`: `task_vm_info` (`phys_footprint`, its lifetime peak, internal, compressed);
/// - `heap`: `malloc_zone_statistics` over all zones (bytes in use, bytes the zones hold);
/// - `regions`: the address space by tag (VirtualMemoryMap);
/// - `metal`: `MTLDevice.currentAllocatedSize` and the FrameSink's three buffers;
/// - `engine`: the engine's own `library.memory` answer, untouched (caches, GPU buffers and, from
///   the bridge, the resident stages of the preview and the window).
nonisolated enum MemoryMapReport {

    static func line(moment: String, elapsed: Double, engineJSON: String, frames: FrameSink) -> String {
        let device   = frames.buffer(at: 0).device
        let engine   = (try? JSONSerialization.jsonObject(with: Data(engineJSON.utf8))) ?? engineJSON
        let regions  = VirtualMemoryMap.byTag().mapValues { total in
            [
                "regions" : total.regions,
                "virtual" : total.virtual,
                "resident": total.resident,
                "dirty"   : total.dirty,
                "swapped" : total.swapped,
                "reusable": total.reusable,
            ]
        }

        let object: [String: Any] = [
            "moment"  : moment,
            "elapsedS": elapsed,
            "thermal" : FootprintSampler.name(of: ProcessInfo.processInfo.thermalState),
            "footprint": footprint(),
            "heap"    : heap(),
            "regions" : regions,
            "metal"   : [
                "currentAllocatedSize": device.currentAllocatedSize,
                "frameSinkBuffers"    : frames.buffer(at: 0).length * FrameSink.bufferCount,
                "recommendedMax"      : device.recommendedMaxWorkingSetSize,
            ],
            "engine"  : engine,
        ]

        let data = (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])) ?? Data()
        return "MM " + String(decoding: data, as: UTF8.self)
    }

    static func footprint() -> [String: UInt64] {
        var info  = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)

        // task_info fills `count` words of the struct it was given; `count` is its size in words.
        let status = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { words in
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), words, &count)
            }
        }

        guard status == KERN_SUCCESS else { return [:] }

        return [
            "physFootprint"    : info.phys_footprint,
            "physFootprintPeak": UInt64(bitPattern: info.ledger_phys_footprint_peak),
            "internal"         : info.internal,
            "compressed"       : info.compressed,
            "external"         : info.external,
            "residentSize"     : info.resident_size,
            "availableBytes"   : UInt64(os_proc_available_memory()),
        ]
    }

    static func heap() -> [String: UInt64] {
        var statistics = malloc_statistics_t()
        malloc_zone_statistics(nil, &statistics)

        return [
            "blocksInUse"  : UInt64(statistics.blocks_in_use),
            "sizeInUse"    : UInt64(statistics.size_in_use),
            "sizeAllocated": UInt64(statistics.size_allocated),
        ]
    }
}
