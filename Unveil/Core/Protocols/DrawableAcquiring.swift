//
//  DrawableAcquiring.swift
//  Unveil
//

/// DrawableAcquiring starts one acquisition without blocking main. Its completion is delivered
/// asynchronously on main exactly once, including when the surface cannot provide a drawable.
nonisolated protocol DrawableAcquiring: Sendable {

    @MainActor
    func acquire(completion: @escaping @MainActor @Sendable (AcquiredDrawable?) -> Void)
}
