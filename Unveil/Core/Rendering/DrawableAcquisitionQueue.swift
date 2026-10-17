//
//  DrawableAcquisitionQueue.swift
//  Unveil
//

import QuartzCore

/// DrawableAcquisitionQueue confines the blocking nextDrawable call to one dedicated serial queue.
/// CanvasView reserves one acquisition through GPU completion and changes drawableSize on main
/// only before that acquisition starts. The worker never changes layer configuration or view
/// state. The unchecked conformance covers the retained layer reference, whose only worker use
/// is nextDrawable. Explicit drawableSize changes never overlap acquisition or GPU use.
/// The bounded dispatch handoff retains closures so the blocking call stays off main; it reuses
/// pixel buffers but does not promise allocation-free scheduling.
nonisolated final class DrawableAcquisitionQueue: DrawableAcquiring, @unchecked Sendable {

    private let layer: CAMetalLayer
    private let queue = DispatchQueue(label: "com.unveil.drawable", qos: .userInteractive)

    init(layer: CAMetalLayer) {
        self.layer = layer
    }

    /// acquire hands the drawable to the caller once. It schedules no retries or recurring work.
    @MainActor
    func acquire(completion: @escaping @MainActor @Sendable (AcquiredDrawable?) -> Void) {
        queue.async { [self] in
            let result = autoreleasepool {
                layer.nextDrawable().map { AcquiredDrawable(drawable: $0) }
            }

            DispatchQueue.main.async {
                completion(result)
            }
        }
    }
}
