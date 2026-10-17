//! Modified for Unveil (2026-10-17): opt-in allocation lifetime observations for RAW development.
//! The observer never owns pixels. Capacity counts allocator payload, not resident or retained pages.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone, Copy, Debug)]
pub enum Change {
    Acquire,
    Transfer,
    Release,
    Handoff,
    Checkpoint,
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub id: u64,
    pub owner: &'static str,
    pub payload: usize,
    pub capacity: usize,
    pub change: Change,
}

type Sink = fn(Event);
pub trait Capacity {
    fn sizes(&self) -> (usize, usize);
}
impl<T> Capacity for Vec<T> {
    fn sizes(&self) -> (usize, usize) {
        (self.len().saturating_mul(size_of::<T>()), self.capacity().saturating_mul(size_of::<T>()))
    }
}
impl<T> Capacity for crate::Buffer<T> {
    fn sizes(&self) -> (usize, usize) {
        (self.len().saturating_mul(size_of::<T>()), self.capacity().saturating_mul(size_of::<T>()))
    }
}
static SINK: OnceLock<Sink> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Install the process observer once. Disabled observations read no clock and allocate nothing.
pub fn set_sink(sink: Sink) {
    let _ = SINK.set(sink);
}

pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}

/// A lifetime marker, separate from the allocation it observes. Drop it just after freeing the
/// allocation; use `handoff` when ownership leaves the measured scope without freeing storage.
pub struct Allocation(Option<Event>);

impl Allocation {
    pub fn new(owner: &'static str, payload: usize, capacity: usize) -> Self {
        if !ENABLED.load(Ordering::Relaxed) {
            return Self(None);
        }
        let event = Event { id: NEXT.fetch_add(1, Ordering::Relaxed), owner, payload, capacity, change: Change::Acquire };
        emit(event);
        Self(Some(event))
    }

    pub fn of(owner: &'static str, data: &impl Capacity) -> Self {
        let (payload, capacity) = data.sizes();
        Self::new(owner, payload, capacity)
    }

    pub fn transfer(&mut self, owner: &'static str) {
        if let Some(event) = self.0.as_mut() {
            event.owner = owner;
            event.change = Change::Transfer;
            emit(*event);
        }
    }

    pub fn handoff(mut self) {
        if let Some(mut event) = self.0.take() {
            event.change = Change::Handoff;
            emit(event);
        }
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        if let Some(mut event) = self.0.take() {
            event.change = Change::Release;
            emit(event);
        }
    }
}

pub fn checkpoint(owner: &'static str) {
    if ENABLED.load(Ordering::Relaxed) {
        emit(Event { id: 0, owner, payload: 0, capacity: 0, change: Change::Checkpoint });
    }
}

fn emit(event: Event) {
    if let Some(sink) = SINK.get() {
        sink(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());

    #[test]
    fn observes_capacity_transfer_and_handoff_without_owning_pixels() {
        set_sink(|e| {
            if matches!(e.owner, "off" | "source" | "consumer" | "temporary") {
                EVENTS.lock().unwrap().push(e);
            }
        });
        let mut data = Vec::<u32>::with_capacity(21);
        data.extend([1, 2, 3]);
        let inactive = Allocation::of("off", &data);
        drop(inactive);
        assert!(EVENTS.lock().unwrap().is_empty());
        set_enabled(true);
        let mut a = Allocation::of("source", &data);
        a.transfer("consumer");
        a.handoff();
        let b = Allocation::of("temporary", &data);
        drop(b);
        set_enabled(false);
        let events = EVENTS.lock().unwrap();
        assert_eq!(events.len(), 5);
        assert_eq!((events[0].payload, events[0].capacity), (12, data.capacity() * 4));
        assert_eq!(events[0].id, events[2].id);
        assert!(matches!(events[1].change, Change::Transfer));
        assert!(matches!(events[2].change, Change::Handoff));
        assert!(matches!(events[4].change, Change::Release));
        assert_eq!(data, [1, 2, 3]);
    }
}
