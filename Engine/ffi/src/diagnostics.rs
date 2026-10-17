//! Read-only diagnostics of the bridge: what the engine loaded, how long each render took and what
//! the resident stages hold. They watch the engine and never change what it does.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use lightcraft_engine::Session;
use serde_json::{Value, json};

unsafe extern "C" {
    // libSystem on iOS and macOS: a monotonic or per-thread clock in nanoseconds.
    fn clock_gettime_nsec_np(clock_id: u32) -> u64;
}

const CLOCK_UPTIME_RAW: u32 = 8;
const CLOCK_THREAD_CPUTIME_ID: u32 = 16;

/// uptime_ns is the monotonic clock the app reads with `clock_gettime_nsec_np(CLOCK_UPTIME_RAW)` too,
/// so an engine event and an app event can be placed on one timeline.
pub(crate) fn uptime_ns() -> u64 {
    // SAFETY: a plain libSystem call with a valid clock id and no pointers.
    unsafe { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }
}

fn thread_cpu_ns() -> u64 {
    // SAFETY: as uptime_ns; the thread clock reads the calling thread's CPU time.
    unsafe { clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) }
}

/// ENABLED is the one switch of this module. It is off in every normal launch and set only by the
/// app's measurement options (`unveil.diagnostics`), so a product run reads no clock and keeps no log.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// LOAD_LOG_CAP bounds the load log: the newest events win, so a forgotten switch never grows memory.
const LOAD_LOG_CAP: usize = 512;

pub(crate) fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// set_enabled turns the diagnostics on or off; turning them off empties the load log.
pub(crate) fn set_enabled(enabled: bool) -> Value {
    ENABLED.store(enabled, Ordering::Relaxed);
    lightcraft_gpu::set_pool_diagnostics(enabled);
    if !enabled {
        lock(&LOADS).clear();
    }
    json!({ "enabled": enabled })
}

/// Span is a started stopwatch: wall time and the calling thread's CPU time, finished on one thread.
/// Started while the diagnostics are off it is inert: no clock is read and `record` ignores it.
pub(crate) struct Span {
    start_ns: u64,
    cpu_ns: u64,
    active: bool,
}

impl Span {
    pub fn start() -> Span {
        if !is_enabled() {
            return Span { start_ns: 0, cpu_ns: 0, active: false };
        }
        Span { start_ns: uptime_ns(), cpu_ns: thread_cpu_ns(), active: true }
    }

    fn finish(&self) -> (u64, u64) {
        (uptime_ns() - self.start_ns, thread_cpu_ns() - self.cpu_ns)
    }
}

/// LoadEvent is one call of a file hook the engine made: a decode, an import probe or an embedded
/// preview read. `width` and `height` are the decoded size, 0 for the others.
struct LoadEvent {
    kind: &'static str,
    name: String,
    max_edge: usize,
    width: usize,
    height: usize,
    start_ns: u64,
    wall_ns: u64,
    cpu_ns: u64,
    ok: bool,
}

static LOADS: Mutex<VecDeque<LoadEvent>> = Mutex::new(VecDeque::new());

/// push_bounded appends `item` and drops the oldest entries beyond `cap`.
fn push_bounded<T>(queue: &mut VecDeque<T>, item: T, cap: usize) {
    while queue.len() >= cap {
        queue.pop_front();
    }
    queue.push_back(item);
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn file_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_owned()
}

fn log_load(kind: &'static str, path: &str, max_edge: usize, size: (usize, usize), span: &Span, ok: bool) {
    if !span.active {
        return;
    }
    let (wall_ns, cpu_ns) = span.finish();
    let event = LoadEvent {
        kind,
        name: file_name(path),
        max_edge,
        width: size.0,
        height: size.1,
        start_ns: span.start_ns,
        wall_ns,
        cpu_ns,
        ok,
    };
    push_bounded(&mut lock(&LOADS), event, LOAD_LOG_CAP);
}

/// instrument_loads wraps the session's file hooks so every decode, probe and embedded preview read is
/// logged (`unveil.loads`). The wrappers call the original hook and return its result untouched.
pub(crate) fn instrument_loads(session: &mut Session) {
    if let Some(inner) = session.media.file_loader.take() {
        session.media.file_loader = Some(Arc::new(move |path: &str, max_edge: usize| {
            let span = Span::start();
            let result = inner(path, max_edge);
            let size = result.as_ref().map_or((0, 0), |(image, _)| (image.width, image.height));
            log_load("decode", path, max_edge, size, &span, result.is_ok());
            result
        }));
    }
    if let Some(inner) = session.media.file_probe.take() {
        session.media.file_probe = Some(Arc::new(move |path: &str| {
            let span = Span::start();
            let result = inner(path);
            log_load("probe", path, 0, (0, 0), &span, result.is_ok());
            result
        }));
    }
    if let Some(inner) = session.media.preview_loader.take() {
        session.media.preview_loader = Some(Arc::new(move |path: &str, max_edge: usize| {
            let span = Span::start();
            let result = inner(path, max_edge);
            let size = result.as_ref().map_or((0, 0), |image| (image.width, image.height));
            log_load("embedded", path, max_edge, size, &span, result.is_some());
            result
        }));
    }
}

/// loads_json returns the file hook events since the last call and clears the log.
pub(crate) fn loads_json() -> Value {
    let events = std::mem::take(&mut *lock(&LOADS));
    let rows: Vec<Value> = events
        .iter()
        .map(|e| {
            json!({
                "kind": e.kind, "name": e.name, "maxEdge": e.max_edge, "width": e.width,
                "height": e.height, "startNs": e.start_ns, "wallNs": e.wall_ns, "cpuNs": e.cpu_ns, "ok": e.ok,
            })
        })
        .collect();
    json!({ "loads": rows })
}

/// Category is a render's kind: draft or full, whole frame or window.
#[derive(Clone, Copy)]
pub(crate) struct Category(usize);

const CATEGORY_NAMES: [&str; 4] = ["fit-draft", "fit-full", "window-draft", "window-full"];

impl Category {
    pub fn of(draft: bool, window: bool) -> Category {
        Category(usize::from(window) * 2 + usize::from(!draft))
    }
}

#[derive(Clone, Copy, Default)]
struct Stat {
    count: u64,
    wall_ns: u64,
    cpu_ns: u64,
}

/// Stats are the totals per category of what the render thread spent in the render itself, in the
/// window's small histogram frame, and what the engine thread spent delivering the frame (the
/// callback: histogram copy and the app's copy into the sink).
#[derive(Default)]
struct Stats {
    render: [Stat; 4],
    histogram: [Stat; 4],
    deliver: [Stat; 4],
    output: [OutputStat; 4],
}

#[derive(Clone, Copy, Default)]
struct OutputStat {
    mapped: u64,
    owned: u64,
    mapped_pixel_bytes: u64,
    physical_pixels: u64,
    display_pixels: u64,
}

#[derive(Clone, Copy)]
pub(crate) enum Part {
    Render,
    Histogram,
    Deliver,
}

static STATS: Mutex<Stats> = Mutex::new(Stats {
    output: [OutputStat { mapped: 0, owned: 0, mapped_pixel_bytes: 0, physical_pixels: 0, display_pixels: 0 };
        4],
    render: [Stat { count: 0, wall_ns: 0, cpu_ns: 0 }; 4],
    histogram: [Stat { count: 0, wall_ns: 0, cpu_ns: 0 }; 4],
    deliver: [Stat { count: 0, wall_ns: 0, cpu_ns: 0 }; 4],
});

/// record_output counts successful render results before delivery or stale-result rejection.
/// Mapped pixel bytes measure payload that avoided an intermediate CPU image, not live memory.
pub(crate) fn record_output(
    category: Category,
    mapped: bool,
    physical: (usize, usize),
    display: (usize, usize),
) {
    if !is_enabled() {
        return;
    }
    let mut stats = lock(&STATS);
    let stat = &mut stats.output[category.0];
    let pixels = physical.0 * physical.1;
    stat.physical_pixels += pixels as u64;
    stat.display_pixels += (display.0 * display.1) as u64;
    if mapped {
        stat.mapped += 1;
        stat.mapped_pixel_bytes += (pixels * 4) as u64;
    } else {
        stat.owned += 1;
    }
}

/// record adds a finished span to the totals of its category and part.
pub(crate) fn record(category: Category, part: Part, span: &Span) {
    if !span.active {
        return;
    }
    let (wall_ns, cpu_ns) = span.finish();
    let mut stats = lock(&STATS);
    let stat = match part {
        Part::Render => &mut stats.render[category.0],
        Part::Histogram => &mut stats.histogram[category.0],
        Part::Deliver => &mut stats.deliver[category.0],
    };
    stat.count += 1;
    stat.wall_ns += wall_ns;
    stat.cpu_ns += cpu_ns;
}

/// renders_json returns the totals so far and, with `reset`, starts them again from zero.
pub(crate) fn renders_json(reset: bool) -> Value {
    let mut stats = lock(&STATS);
    let part = |stats: &[Stat; 4]| -> Value {
        let rows: Vec<Value> = stats
            .iter()
            .zip(CATEGORY_NAMES)
            .map(|(s, name)| json!({ "category": name, "count": s.count, "wallNs": s.wall_ns, "cpuNs": s.cpu_ns }))
            .collect();
        Value::Array(rows)
    };
    let out = json!({
        "render": part(&stats.render), "histogram": part(&stats.histogram), "deliver": part(&stats.deliver),
        "output": stats.output.iter().zip(CATEGORY_NAMES).map(|(s, name)| json!({
            "category": name, "mapped": s.mapped, "owned": s.owned, "mappedPixelBytes": s.mapped_pixel_bytes,
            "physicalPixels": s.physical_pixels, "displayPixels": s.display_pixels,
        })).collect::<Vec<_>>(),
    });
    if reset {
        *stats = Stats::default();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_load_log_keeps_only_the_newest_events() {
        let mut queue = VecDeque::new();
        for n in 0..(LOAD_LOG_CAP + 10) {
            push_bounded(&mut queue, n, LOAD_LOG_CAP);
        }
        assert_eq!(queue.len(), LOAD_LOG_CAP);
        assert_eq!(queue.front(), Some(&10));
        assert_eq!(queue.back(), Some(&(LOAD_LOG_CAP + 9)));
    }

    #[test]
    fn a_span_started_while_off_is_inert() {
        assert!(!is_enabled(), "no other test of this binary enables the diagnostics");
        let span = Span::start();
        assert!(!span.active && span.start_ns == 0);
        record(Category::of(true, false), Part::Render, &span);
        assert_eq!(renders_json(false)["render"][0]["count"], 0);
    }
}
