//! Opt-in, bounded allocation observations. These are scoped live payloads, not allocator pages.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, OnceLock};

use lightcraft_engine::files::decode_memory::{self as trace, Change, Event};
use serde_json::{Value, json};

const MAX_EVENTS: usize = 2048;
const MAX_LIVE: usize = 16384;

#[derive(Default)]
struct State {
    live: BTreeMap<u64, Event>,
    events: VecDeque<Value>,
    peak: usize,
    current: usize,
    owners: BTreeMap<&'static str, (usize, usize, usize)>,
    last_ns: u64,
    dropped: usize,
    incomplete: bool,
}

impl State {
    fn observe(&mut self, event: Event, now: u64) {
        if let Some(old) = self.live.remove(&event.id) {
            self.current -= old.capacity;
            if let Some(entry) = self.owners.get_mut(old.owner) {
                entry.0 -= 1;
                entry.1 -= old.payload;
                entry.2 -= old.capacity;
                if entry.0 == 0 {
                    self.owners.remove(old.owner);
                }
            }
        }
        match event.change {
            Change::Acquire | Change::Transfer => {
                if self.live.len() < MAX_LIVE || self.live.contains_key(&event.id) {
                    self.live.insert(event.id, event);
                    self.current += event.capacity;
                    let entry = self.owners.entry(event.owner).or_insert((0, 0, 0));
                    entry.0 += 1;
                    entry.1 += event.payload;
                    entry.2 += event.capacity;
                } else {
                    self.incomplete = true;
                }
            }
            Change::Checkpoint => {}
            Change::Release | Change::Handoff => {
                self.live.remove(&event.id);
            }
        }
        self.peak = self.peak.max(self.current);
        if !matches!(event.change, Change::Checkpoint)
            && event.capacity < 1 << 20
            && now.saturating_sub(self.last_ns) < 100_000_000
        {
            return;
        }
        self.last_ns = now;
        let gpu = lightcraft_gpu::memory();
        let row = json!({
            "timeNs": now, "id": event.id, "owner": event.owner,
            "change": format!("{:?}", event.change), "payload": event.payload, "capacity": event.capacity,
            "live": self.owners(), "scopedPeak": self.peak,
            "gpu": { "allocated": gpu.allocated, "pooled": gpu.pooled, "retired": gpu.retired },
        });
        if self.events.len() == MAX_EVENTS {
            self.events.pop_front();
            self.dropped += 1;
        }
        self.events.push_back(row);
    }

    fn owners(&self) -> &BTreeMap<&'static str, (usize, usize, usize)> {
        &self.owners
    }
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(Mutex::default)
}

pub(crate) fn command(enabled: Option<bool>) -> Value {
    if let Some(enabled) = enabled {
        trace::set_enabled(false);
        *state().lock().unwrap_or_else(|e| e.into_inner()) = State::default();
        trace::set_sink(|event| {
            state().lock().unwrap_or_else(|e| e.into_inner()).observe(event, crate::diagnostics::uptime_ns());
        });
        trace::set_enabled(enabled);
    }
    let mut s = state().lock().unwrap_or_else(|e| e.into_inner());
    json!({
        "events": s.events.drain(..).collect::<Vec<_>>(), "scopedPeak": s.peak,
        "live": s.owners(), "droppedEvents": s.dropped, "incomplete": s.incomplete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfers_do_not_duplicate_capacity_and_handoffs_end_the_observed_scope() {
        let mut s = State::default();
        let mut e = Event { id: 1, owner: "old", capacity: 50, payload: 20, change: Change::Acquire };
        s.observe(e, 1);
        e.owner = "new";
        e.change = Change::Transfer;
        s.observe(e, 2);
        assert_eq!(s.peak, 50);
        assert_eq!(s.owners().get("new"), Some(&(1, 20, 50)));
        assert!(!s.owners().contains_key("old"));
        e.change = Change::Handoff;
        s.observe(e, 3);
        assert!(s.live.is_empty());
    }

    #[test]
    fn live_observations_are_bounded_and_overflow_is_explicit() {
        let mut s = State::default();
        for id in 0..=MAX_LIVE as u64 {
            s.observe(Event { id, owner: "test", capacity: 1, payload: 1, change: Change::Acquire }, 0);
        }
        assert_eq!(s.live.len(), MAX_LIVE);
        assert!(s.incomplete);
        assert!(s.events.is_empty());
    }
}
