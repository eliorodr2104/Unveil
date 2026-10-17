//
//  ControlledDrawableSource.swift
//  UnveilTests
//

import Metal
import QuartzCore
import Testing
@testable import Unveil

/// ControlledDrawableSource holds the canvas's callback while acquiring a real drawable on the
/// production queue. Tests can then release that drawable, or nil, after intervening UI changes.
@MainActor
final class ControlledDrawableSource: DrawableAcquiring {

    private let upstream: DrawableAcquisitionQueue
    private var pending: (@MainActor @Sendable (AcquiredDrawable?) -> Void)?

    private(set) var requestCount   = 0
    private(set) var presentedCount = 0
    private(set) var prepared : AcquiredDrawable?
    private(set) var delivered: AcquiredDrawable?

    // any: Metal exposes textures as protocol existentials. Retained before presentation.
    private(set) var deliveredTexture: (any MTLTexture)?

    init(layer: CAMetalLayer) {
        upstream = DrawableAcquisitionQueue(layer: layer)
    }

    func acquire(completion: @escaping @MainActor @Sendable (AcquiredDrawable?) -> Void) {
        #expect(pending == nil)
        requestCount += 1
        pending = completion
    }

    func prepareDrawable() async throws {
        let result: AcquiredDrawable? = await withCheckedContinuation { continuation in
            upstream.acquire { continuation.resume(returning: $0) }
        }
        let drawable: AcquiredDrawable = try #require(result)
        prepared = drawable
    }

    func completeWithNil() throws {
        let completion = try #require(pending)
        pending = nil
        completion(nil)
    }

    func completeWithDrawable() throws {
        let completion = try #require(pending)
        let result = try #require(prepared)
        pending   = nil
        prepared  = nil
        delivered = result
        deliveredTexture = result.drawable.texture
        result.drawable.addPresentedHandler { [weak self] drawable in
            guard drawable.presentedTime > 0 else { return }
            DispatchQueue.main.async { self?.presentedCount += 1 }
        }
        completion(result)
    }

    func cancelPending() {
        pending?(nil)
        pending   = nil
        prepared  = nil
        delivered = nil
        deliveredTexture = nil
    }
}
