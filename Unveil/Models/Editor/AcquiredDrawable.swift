//
//  AcquiredDrawable.swift
//  Unveil
//

import QuartzCore

/// AcquiredDrawable transfers a retained Metal drawable from the acquisition queue to main.
/// The worker never accesses it after the transfer. Main alone encodes and presents it, or drops
/// it if the surface changed. The unchecked conformance covers this one-way ownership handoff;
/// neither the drawable nor its texture may be used concurrently by the worker and main.
nonisolated struct AcquiredDrawable: @unchecked Sendable {

    // any: Metal vends drawables only as protocol existentials.
    let drawable: any CAMetalDrawable
}
