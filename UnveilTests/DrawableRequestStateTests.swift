//
//  DrawableRequestStateTests.swift
//  UnveilTests
//

import Testing
@testable import Unveil

struct DrawableRequestStateTests {

    @Test
    func demandCoalescesUntilGPUCompletion() throws {
        var state = DrawableRequestState()
        #expect(state.beginAcquisition() == nil)
        state.invalidate(isAttached: true)
        let tokenAcquisition = state.beginAcquisition()
        let token = try #require(tokenAcquisition)

        for _ in 0 ..< 100 { state.markDirty() }

        #expect(state.beginAcquisition() == nil)
        #expect(state.receive(token, hasDrawable: true) == true)
        #expect(state.beginAcquisition() == nil)
        #expect(state.finish(token) == true)
        #expect(state.canAcquire)
    }

    @Test
    func reattachmentWaitsForTheOldAcquisition() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let oldAcquisition = state.beginAcquisition()
        let old = try #require(oldAcquisition)

        state.invalidate(isAttached: false)
        state.invalidate(isAttached: true)
        #expect(!state.isCurrent(old))
        #expect(state.beginAcquisition() == nil)
        #expect(state.receive(old, hasDrawable: true) == false)

        let currentAcquisition = state.beginAcquisition()
        let current = try #require(currentAcquisition)
        #expect(state.receive(old, hasDrawable: true) == false)
        #expect(state.beginAcquisition() == nil)
        #expect(state.receive(current, hasDrawable: true) == true)
    }

    @Test
    func resizeRejectsTheOldSurfaceWithoutStartingAnotherAcquisition() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let oldAcquisition = state.beginAcquisition()
        let old = try #require(oldAcquisition)

        state.invalidate(isAttached: true)
        #expect(state.beginAcquisition() == nil)
        #expect(state.receive(old, hasDrawable: true) == false)
        #expect(state.canAcquire)
    }

    @Test
    func aLateGPUCompletionCannotReleaseANewerReservation() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let oldAcquisition = state.beginAcquisition()
        let old = try #require(oldAcquisition)
        #expect(state.receive(old, hasDrawable: true) == true)

        state.invalidate(isAttached: true)
        #expect(state.beginAcquisition() == nil)
        #expect(state.finish(old) == true)
        let currentAcquisition = state.beginAcquisition()
        let current = try #require(currentAcquisition)
        #expect(state.finish(old) == false)
        #expect(state.beginAcquisition() == nil)
        #expect(state.receive(current, hasDrawable: true) == true)
    }

    @Test
    func aFinalFrameRecoversFromNilWithoutAnotherInput() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let firstAcquisition = state.beginAcquisition()
        let first = try #require(firstAcquisition)
        #expect(state.receive(first, hasDrawable: false) == false)
        #expect(!state.canAcquire)
        let retry = try #require(state.retry)
        #expect(retry.delay == 0.05)

        #expect(state.resumeRetry(retry) == true)
        let finalAcquisition = state.beginAcquisition()
        let final = try #require(finalAcquisition)
        #expect(state.receive(final, hasDrawable: true) == true)
        #expect(state.finish(final) == true)
        #expect(state.retry == nil)
    }

    @Test
    func repeatedNilStopsAfterThreeDelayedRetries() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)

        for delay in [0.05, 0.1, 0.2] {
            let tokenAcquisition = state.beginAcquisition()
            let token = try #require(tokenAcquisition)
            #expect(state.receive(token, hasDrawable: false) == false)
            let retry = try #require(state.retry)
            #expect(retry.delay == delay)
            #expect(!state.canAcquire)
            #expect(state.resumeRetry(retry) == true)
            #expect(state.resumeRetry(retry) == false)
        }

        let lastAcquisition = state.beginAcquisition()
        let last = try #require(lastAcquisition)
        #expect(state.receive(last, hasDrawable: false) == false)
        #expect(state.retry == nil)
        #expect(!state.canAcquire)

        state.markDirty()
        #expect(state.canAcquire)
    }

    @Test
    func freshDemandAndReparentingCancelScheduledRetries() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let firstAcquisition = state.beginAcquisition()
        let first = try #require(firstAcquisition)
        #expect(state.receive(first, hasDrawable: false) == false)
        let old = try #require(state.retry)

        state.markDirty()
        #expect(state.resumeRetry(old) == false)
        let currentAcquisition = state.beginAcquisition()
        let current = try #require(currentAcquisition)
        #expect(state.receive(current, hasDrawable: false) == false)
        let detached = try #require(state.retry)

        state.invalidate(isAttached: false)
        #expect(state.resumeRetry(detached) == false)
        #expect(!state.canAcquire)
        state.invalidate(isAttached: true)
        #expect(state.resumeRetry(detached) == false)
        #expect(state.canAcquire)
    }

    @Test
    func encodingFailureAlsoHasABoundedRecovery() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let tokenAcquisition = state.beginAcquisition()
        let token = try #require(tokenAcquisition)
        #expect(state.receive(token, hasDrawable: true) == true)
        #expect(state.finish(token, succeeded: false) == true)
        #expect(!state.canAcquire)
        let retry = try #require(state.retry)
        #expect(state.resumeRetry(retry) == true)
        #expect(state.canAcquire)
    }

    @Test
    func aReadyDrawableWaitsForATickWithoutStartingAnotherAcquisition() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let acquired = state.beginAcquisition()
        let token = try #require(acquired)
        #expect(state.receive(token, hasDrawable: true, waitForTick: true) == true)
        #expect(!state.isRendering)
        #expect(!state.canAcquire)
        #expect(state.beginRendering(token) == true)
        #expect(state.isRendering)
        #expect(state.beginRendering(token) == false)
        #expect(state.finish(token) == true)
        #expect(state.canAcquire)
    }

    @Test
    func idleCanReleaseAReadyDrawableWithoutRetrying() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let acquired = state.beginAcquisition()
        let token = try #require(acquired)
        #expect(state.receive(token, hasDrawable: true, waitForTick: true) == true)
        #expect(state.discardReady(token) == true)
        #expect(state.beginRendering(token) == false)
        #expect(state.retry == nil)
        #expect(state.canAcquire)
    }

    @Test
    func anOldReadyDrawableCannotRenderOnANewSurface() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let acquired = state.beginAcquisition()
        let token = try #require(acquired)
        #expect(state.receive(token, hasDrawable: true, waitForTick: true) == true)
        state.invalidate(isAttached: true)
        #expect(state.beginRendering(token) == false)
        #expect(state.discardReady(token) == true)
        #expect(state.canAcquire)
    }

    @Test
    func readyDiscardCannotReleaseAnAcquisitionOrGPUPass() throws {
        var state = DrawableRequestState()
        state.invalidate(isAttached: true)
        let acquired = state.beginAcquisition()
        let token = try #require(acquired)
        #expect(state.discardReady(token) == false)
        #expect(state.receive(token, hasDrawable: true) == true)
        #expect(state.discardReady(token) == false)
        #expect(!state.canAcquire)
        #expect(state.finish(token) == true)
    }

}
