//
//  PendingFrameIntervals.swift
//  Unveil
//

/// PendingFrameIntervals is the bookkeeping behind the intervals that end when a frame is presented
/// (`SliderToFrame`, `OpenToFirstFrame`): each waits for a generation, and a presented generation
/// ends every interval at or below it, because the engine numbers its renders in order and a frame
/// above an interval's own generation means its render was overtaken and will never be shown.
///
/// It is a plain value with no clock and no signposter, so the matching rule is unit tested as is.
/// Signposts keeps one behind a Mutex: requests add on the caller's thread, presents drain on
/// Metal's thread. `lastPresented` lets an interval added after its frame was shown end at once.
nonisolated struct PendingFrameIntervals<Token> {

    struct Entry {

        let generation: UInt64
        let token     : Token
    }

    private(set) var lastPresented: UInt64  = 0
    private(set) var entries      : [Entry] = []

    /// add records `token` as waiting for `generation`. It returns false, and records nothing, when
    /// that generation (or a later one) was already presented: the caller ends the interval itself.
    mutating func add(_ token: Token, waitingFor generation: UInt64) -> Bool {
        guard generation > lastPresented else { return false }

        entries.append(Entry(generation: generation, token: token))
        return true
    }

    /// presented removes and returns every entry at or below `generation`, in the order they were
    /// added. An entry is returned once: a later present no longer sees it.
    mutating func presented(_ generation: UInt64) -> [Entry] {
        lastPresented = max(lastPresented, generation)

        guard entries.contains(where: { $0.generation <= generation }) else { return [] }

        let ended = entries.filter { $0.generation <= generation }
        entries.removeAll { $0.generation <= generation }
        return ended
    }
}
