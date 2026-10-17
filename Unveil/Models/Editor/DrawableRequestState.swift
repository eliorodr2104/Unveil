//
//  DrawableRequestState.swift
//  Unveil
//

/// DrawableRequestState reserves one drawable acquisition through its GPU completion.
/// Surface epochs reject stale callbacks without releasing a physical operation early. Failures
/// allow three delayed retries, so a final dirty frame can recover without polling while idle.
nonisolated struct DrawableRequestState {

    struct Retry: Equatable, Sendable {

        let epoch   : UInt64
        let revision: UInt64
        let attempt : Int

        var delay: Double { 0.05 * Double(1 << (attempt - 1)) }
    }

    struct Token: Equatable, Sendable {

        let serial: UInt64
        let epoch : UInt64
    }

    private enum Phase {

        case acquiring
        case ready
        case rendering
    }

    private var epoch          : UInt64 = 0
    private var serial         : UInt64 = 0
    private var revision       : UInt64 = 0
    private var blockedRevision: UInt64?
    private var isAttached     = false
    private var active         : Token?
    private var phase          : Phase?
    private var retryCount     = 0

    private(set) var retry: Retry?

    var canAcquire: Bool {
        isAttached && active == nil && blockedRevision != revision
    }

    var isRendering: Bool {
        phase == .rendering && active?.epoch == epoch && isAttached
    }

    /// isCurrent validates callbacks that can outlive GPU completion, such as presentation.
    func isCurrent(_ token: Token) -> Bool {
        isAttached && token.epoch == epoch
    }

    /// invalidate rejects the old surface but keeps its reservation until the operation returns.
    mutating func invalidate(isAttached: Bool) {
        epoch &+= 1
        self.isAttached = isAttached
        blockedRevision = nil
        retryCount      = 0
        retry           = nil
    }

    /// markDirty cancels an older retry and gives new demand its own finite recovery budget.
    mutating func markDirty() {
        revision &+= 1
        blockedRevision = nil
        retryCount      = 0
        retry           = nil
    }

    /// beginAcquisition reserves the slot before any off-main work can start.
    mutating func beginAcquisition() -> Token? {
        guard canAcquire else { return nil }

        serial &+= 1
        let token = Token(serial: serial, epoch: epoch)
        active = token
        phase  = .acquiring
        return token
    }

    /// receive accepts one current drawable, or releases a stale or failed acquisition.
    mutating func receive(_ token: Token, hasDrawable: Bool, waitForTick: Bool = false) -> Bool {
        guard active == token, phase == .acquiring else { return false }

        guard isCurrent(token) else {
            active = nil
            phase  = nil
            return false
        }
        guard hasDrawable else {
            active = nil
            phase  = nil
            failedAttempt()
            return false
        }
        phase = waitForTick ? .ready : .rendering
        return true
    }

    /// beginRendering consumes a prepared drawable only for its current surface.
    mutating func beginRendering(_ token: Token) -> Bool {
        guard active == token, phase == .ready, isCurrent(token) else { return false }
        phase = .rendering
        return true
    }

    /// discardReady releases a drawable that was acquired but never submitted to the GPU.
    @discardableResult
    mutating func discardReady(_ token: Token) -> Bool {
        guard active == token, phase == .ready else { return false }
        active = nil
        phase = nil
        return true
    }

    /// finish releases only the matching GPU reservation; duplicate completions cannot free a new one.
    @discardableResult
    mutating func finish(_ token: Token, succeeded: Bool = true) -> Bool {
        guard active == token, phase == .rendering else { return false }

        active = nil
        phase  = nil
        if !succeeded, token.epoch == epoch {
            failedAttempt()
        } else if succeeded, token.epoch == epoch {
            retryCount = 0
        }
        return true
    }

    /// resumeRetry consumes only the pending retry for the current surface and demand.
    mutating func resumeRetry(_ retry: Retry) -> Bool {
        guard isAttached, self.retry == retry, active == nil else { return false }

        self.retry      = nil
        blockedRevision = nil
        return true
    }

    private mutating func failedAttempt() {
        blockedRevision = revision
        guard retryCount < 3 else { return }

        retryCount += 1
        retry = Retry(epoch: epoch, revision: revision, attempt: retryCount)
    }
}
