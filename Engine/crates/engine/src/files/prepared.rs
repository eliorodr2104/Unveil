//! One active photo's decoded sensor samples, retained until its first full development.
//! Modified for Unveil (2026-10-14): bounded preparation reuse without retaining CPU RGB sources.

use std::io::Write;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use lightcraft_preview::Hash128;
use lightcraft_raw::{RawData, RawImage};

use super::*;

const MAX_BYTES: usize = 128 << 20;
const MAX_HEADER: usize = 8 << 20;
const PREPARATION_VERSION: u32 = 1;

/// A single interactive open. Replacing or clearing the context invalidates every older token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreparedOpen(u64);

/// Decoder invocations and owned preparation bytes, independent of the number of file-loader calls.
#[derive(Clone, Copy, Debug, Default)]
pub struct PreparedStats {
    pub sample_decodes: usize,
    pub prepared_hits: usize,
    pub content_misses: usize,
    pub oversize_misses: usize,
    pub retained_bytes: usize,
}

struct Open {
    token: PreparedOpen,
    path: String,
    confirmed: bool,
    started: Instant,
    consumed: bool,
}

struct Entry {
    hash: Hash128,
    version: u32,
    samples: RawData,
    header: Box<[u8]>,
}

impl Entry {
    fn bytes(&self) -> usize {
        sample_bytes(&self.samples).saturating_add(self.header.len()).saturating_add(std::mem::size_of::<Self>())
    }
}

#[derive(Default)]
struct State {
    epoch: u64,
    open: Option<Open>,
    entry: Option<Entry>,
    busy: bool,
    leased_bytes: usize,
    stats: PreparedStats,
}

/// PreparedSources owns at most one active photo's samples within a limit capped at 128 MiB.
/// begin_open/confirm_open bracket interactive import and selection; clear handles pressure, close,
/// suspend and explicit reload. Other paths and thumbnails retain nothing. No mutex is held during IO
/// or development; one lease serializes eligible loads and stale leases cannot put their samples back.
#[derive(Default)]
pub struct PreparedSources {
    limit: usize,
    state: Mutex<State>,
    available: Condvar,
}

impl PreparedSources {
    pub fn new(limit: usize) -> Self {
        Self { limit: limit.min(MAX_BYTES), ..Self::default() }
    }

    pub fn begin_open(&self, path: &str) -> PreparedOpen {
        self.begin_open_at(path, Instant::now())
    }

    fn begin_open_at(&self, path: &str, now: Instant) -> PreparedOpen {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.epoch = state.epoch.wrapping_add(1);
        let token = PreparedOpen(state.epoch);
        state.entry = None;
        state.open = Some(Open { token, path: path.into(), confirmed: false, started: now, consumed: false });
        token
    }

    pub fn confirm_open(&self, token: PreparedOpen) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::expire(&mut state, Instant::now());
        let Some(open) = state.open.as_mut().filter(|open| open.token == token) else { return false };
        open.confirmed = true;
        true
    }

    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.epoch = state.epoch.wrapping_add(1);
        state.open = None;
        state.entry = None;
        self.available.notify_all();
    }

    /// Cancel only this open: a late failure must not evict a newer photo's preparation.
    pub fn cancel_open(&self, token: PreparedOpen) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.open.as_ref().is_some_and(|open| open.token == token) {
            state.open = None;
            state.entry = None;
            self.available.notify_all();
        }
    }

    /// Confirm the selected file, or replace the preparation when the selected photo changes.
    pub fn select(&self, path: Option<&str>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::expire(&mut state, Instant::now());
        if let Some(open) = state.open.as_mut().filter(|open| Some(open.path.as_str()) == path) {
            open.confirmed = true;
            return;
        }
        state.epoch = state.epoch.wrapping_add(1);
        state.entry = None;
        state.open =
            path.map(|path| Open { token: PreparedOpen(state.epoch), path: path.into(), confirmed: true, started: Instant::now(), consumed: false });
        self.available.notify_all();
    }

    /// Service the provisional import deadline even when no later loader request arrives.
    pub fn expire_pending(&self) {
        Self::expire(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()), Instant::now());
    }

    pub fn pending_timeout(&self) -> Option<Duration> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .open
            .as_ref()
            .filter(|open| !open.confirmed)
            .map(|open| Duration::from_secs(5).saturating_sub(open.started.elapsed()))
    }

    pub fn stats(&self) -> PreparedStats {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Self::expire(&mut state, Instant::now());
        PreparedStats { retained_bytes: state.leased_bytes + state.entry.as_ref().map_or(0, Entry::bytes), ..state.stats }
    }

    fn expire(state: &mut State, now: Instant) {
        if state.open.as_ref().is_some_and(|open| !open.confirmed && now.saturating_duration_since(open.started) >= Duration::from_secs(5)) {
            state.epoch = state.epoch.wrapping_add(1);
            state.open = None;
            state.entry = None;
        }
    }

    fn lease(&self, path: &str, max_edge: usize) -> Option<Lease<'_>> {
        if max_edge <= 520 {
            return None;
        }
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            Self::expire(&mut state, Instant::now());
            let open = state.open.as_ref().filter(|open| open.path == path && !open.consumed)?;
            let token = open.token;
            if !state.busy {
                state.busy = true;
                let entry = state.entry.take();
                state.leased_bytes = entry.as_ref().map_or(0, Entry::bytes);
                return Some(Lease { sources: self, token, entry, header: None, hash: None, full: max_edge > 2560 });
            }
            state = self.available.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn decoded(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stats.sample_decodes += 1;
    }

    /// load uses prepared samples only for this active path and matching bytes. Full loads consume them.
    pub fn load(&self, path: &str, bytes: Vec<u8>, max_edge: usize) -> Result<(Rgb32f, SourceInfo), String> {
        self.load_output(path, bytes, max_edge, None).map(|(image, _, info)| (image, info))
    }

    pub fn load_pair(&self, path: &str, bytes: Vec<u8>, max_edge: usize, spec: &DenoiseSpec) -> Result<(Rgb32f, Option<Rgb32f>, SourceInfo), String> {
        self.load_output(path, bytes, max_edge, Some(spec))
    }

    fn load_output(
        &self,
        path: &str,
        bytes: Vec<u8>,
        max_edge: usize,
        spec: Option<&DenoiseSpec>,
    ) -> Result<(Rgb32f, Option<Rgb32f>, SourceInfo), String> {
        let background = crate::memory::is_background();
        // Wait on the caller before entering rayon: a waiting load must not occupy a decode worker.
        let mut lease = if background { None } else { self.lease(path, max_edge) };
        rayon::scope(move |_| {
            if lease.is_none() && lightcraft_raw::probe(&bytes).is_some() {
                self.decoded();
            }
            load_bytes_from(std::borrow::Cow::Owned(bytes), max_edge, spec, lease.as_mut())
        })
    }

    /// probe transfers a vendor's already decoded samples into the same slot the first preview uses.
    pub fn probe(&self, path: &str, bytes: &[u8]) -> Result<ProbeInfo, String> {
        let Some(mut lease) = self.lease(path, 2560) else { return probe_bytes(path, bytes) };
        if lightcraft_raw::probe(bytes).is_none() {
            return probe_bytes(path, bytes);
        }
        if lease.entry.as_ref().is_some_and(|entry| entry.hash == lightcraft_preview::hash_bytes(bytes)) {
            let result = lease.decode(bytes).map(|mut raw| {
                let info = raw.info();
                lease.samples_done(&mut raw);
                info
            });
            return probe_bytes_from(path, bytes, Some(result));
        }
        if lease.entry.take().is_some() {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.leased_bytes = 0;
            state.stats.content_misses += 1;
        }
        let result = lightcraft_raw::probe_with_samples(bytes).map(|(info, raw)| {
            if let Some(mut raw) = raw {
                self.decoded();
                lease.hash = Some(lightcraft_preview::hash_bytes(bytes));
                lease.capture_header(&mut raw);
                lease.samples_done(&mut raw);
            }
            info
        });
        probe_bytes_from(path, bytes, Some(result))
    }
}

pub(super) struct Lease<'a> {
    sources: &'a PreparedSources,
    token: PreparedOpen,
    entry: Option<Entry>,
    header: Option<Box<[u8]>>,
    hash: Option<Hash128>,
    full: bool,
}

impl Lease<'_> {
    pub(super) fn decode(&mut self, bytes: &[u8]) -> lightcraft_raw::Result<RawImage> {
        let hash = lightcraft_preview::hash_bytes(bytes);
        self.hash = Some(hash);
        if let Some(entry) = self.entry.take() {
            if entry.hash == hash && entry.version == PREPARATION_VERSION {
                if let Ok(mut raw) = serde_json::from_slice::<RawImage>(&entry.header) {
                    raw.data = entry.samples;
                    self.header = Some(entry.header);
                    self.sources.state.lock().unwrap_or_else(|e| e.into_inner()).stats.prepared_hits += 1;
                    return Ok(raw);
                }
            } else {
                self.sources.state.lock().unwrap_or_else(|e| e.into_inner()).stats.content_misses += 1;
            }
            self.sources.state.lock().unwrap_or_else(|e| e.into_inner()).leased_bytes = 0;
        }
        self.sources.decoded();
        lightcraft_raw::decode(bytes)
    }

    pub(super) fn capture_header(&mut self, raw: &mut RawImage) {
        if self.full || self.header.is_some() {
            return;
        }
        if sample_bytes(&raw.data) > self.sources.limit {
            self.sources.state.lock().unwrap_or_else(|e| e.into_inner()).stats.oversize_misses += 1;
            return;
        }
        // The metadata is serialized with no samples, so retained byte accounting includes every owner.
        // float_roundtrip is enabled by the workspace; reject metadata that cannot round-trip exactly.
        let samples = std::mem::replace(&mut raw.data, RawData::U16(Vec::new()));
        let mut writer = HeaderWriter(Vec::new());
        let encoded = serde_json::to_writer(&mut writer, &raw).is_ok();
        if encoded && serde_json::from_slice::<RawImage>(&writer.0).ok().as_ref() == Some(raw) {
            let header = writer.0.into_boxed_slice();
            let bytes = sample_bytes(&samples).saturating_add(header.len()).saturating_add(std::mem::size_of::<Entry>());
            if bytes <= self.sources.limit {
                self.header = Some(header);
                self.sources.state.lock().unwrap_or_else(|e| e.into_inner()).leased_bytes = bytes;
            } else {
                self.sources.state.lock().unwrap_or_else(|e| e.into_inner()).stats.oversize_misses += 1;
            }
        }
        raw.data = samples;
    }

    /// samples_done moves the samples out before colour and resize allocate their later scratch.
    pub(super) fn samples_done(&mut self, raw: &mut RawImage) {
        let samples = std::mem::replace(&mut raw.data, RawData::U16(Vec::new()));
        let mut state = self.sources.state.lock().unwrap_or_else(|e| e.into_inner());
        state.leased_bytes = 0;
        if self.full
            && let Some(open) = state.open.as_mut().filter(|open| open.token == self.token)
        {
            open.consumed = true;
        }
        if !self.full && state.open.as_ref().is_some_and(|open| open.token == self.token) {
            if let (Some(hash), Some(header)) = (self.hash, self.header.take()) {
                state.entry = Some(Entry { hash, version: PREPARATION_VERSION, samples, header });
            }
        }
        self.header = None;
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        let mut state = self.sources.state.lock().unwrap_or_else(|e| e.into_inner());
        state.leased_bytes = 0;
        state.busy = false;
        self.sources.available.notify_all();
    }
}

fn sample_bytes(samples: &RawData) -> usize {
    match samples {
        RawData::U16(data) => data.capacity().saturating_mul(std::mem::size_of::<u16>()),
        RawData::F32(data) => data.capacity().saturating_mul(std::mem::size_of::<f32>()),
    }
}

struct HeaderWriter(Vec<u8>);

impl Write for HeaderWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len().saturating_add(bytes.len()) > MAX_HEADER {
            return Err(std::io::Error::other("prepared RAW header exceeds its bound"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        crate::tests_xmp::synthetic_dng_with(None, Default::default())
    }

    #[test]
    fn confirmed_samples_survive_idle_but_unconfirmed_imports_expire() {
        for confirmed in [false, true] {
            let sources = PreparedSources::new(MAX_BYTES);
            let now = Instant::now();
            let token = sources.begin_open_at("a.dng", now);
            if confirmed {
                assert!(sources.confirm_open(token));
            }
            sources.load("a.dng", fixture(), 2560).unwrap();
            let mut state = sources.state.lock().unwrap();
            PreparedSources::expire(&mut state, now + Duration::from_secs(60));
            assert_eq!(state.entry.is_some(), confirmed);
        }
    }

    #[test]
    fn pressure_retires_a_lease_without_reinserting_its_samples() {
        let sources = PreparedSources::new(MAX_BYTES);
        let token = sources.begin_open("a.dng");
        assert!(sources.confirm_open(token));
        let bytes = fixture();
        sources.load("a.dng", bytes.clone(), 2560).unwrap();
        let mut lease = sources.lease("a.dng", 2560).unwrap();
        let mut raw = lease.decode(&bytes).unwrap();
        assert!(sources.stats().retained_bytes > 0);
        sources.clear();
        assert!(sources.stats().retained_bytes > 0, "the in-flight lease still owns its samples");
        lease.samples_done(&mut raw);
        assert!(raw.data.is_empty());
        assert_eq!(sources.stats().retained_bytes, 0);
        drop(lease);
        assert_eq!(sources.stats().retained_bytes, 0);
    }

    #[test]
    fn restoring_preparation_moves_the_sample_allocation_without_copying_it() {
        let sources = PreparedSources::new(MAX_BYTES);
        let token = sources.begin_open("a.dng");
        assert!(sources.confirm_open(token));
        let bytes = fixture();
        sources.load("a.dng", bytes.clone(), 2560).unwrap();
        let address = {
            let state = sources.state.lock().unwrap();
            let RawData::U16(data) = &state.entry.as_ref().unwrap().samples else { panic!("u16 fixture") };
            data.as_ptr()
        };
        let mut lease = sources.lease("a.dng", usize::MAX).unwrap();
        let mut raw = lease.decode(&bytes).unwrap();
        let RawData::U16(data) = &raw.data else { panic!("u16 fixture") };
        assert_eq!(data.as_ptr(), address);
        lease.samples_done(&mut raw);
        assert!(raw.data.is_empty());
        assert!(lease.header.is_none());
        assert_eq!(sources.stats().retained_bytes, 0, "released before downstream color/resize runs");
    }

    #[test]
    fn late_cancel_does_not_clear_a_new_selection_and_full_does_not_rearm() {
        let sources = PreparedSources::new(MAX_BYTES);
        let old = sources.begin_open("old.dng");
        sources.select(Some("new.dng"));
        sources.cancel_open(old);
        sources.load("new.dng", fixture(), 2560).unwrap();
        assert!(sources.stats().retained_bytes > 0);
        sources.load("new.dng", fixture(), usize::MAX).unwrap();
        sources.select(Some("new.dng"));
        sources.load("new.dng", fixture(), 2560).unwrap();
        assert_eq!(sources.stats().retained_bytes, 0);
        sources.select(None);
        assert!(sources.state.lock().unwrap().open.is_none());
    }

    #[test]
    fn cached_probe_reuses_samples_and_a_cancelled_lease_cannot_restore_them() {
        let sources = PreparedSources::new(MAX_BYTES);
        let token = sources.begin_open("a.dng");
        let bytes = fixture();
        sources.load("a.dng", bytes.clone(), 2560).unwrap();
        sources.probe("a.dng", &bytes).unwrap();
        assert_eq!(sources.stats().sample_decodes, 1);
        let mut lease = sources.lease("a.dng", 2560).unwrap();
        let mut raw = lease.decode(&bytes).unwrap();
        sources.cancel_open(token);
        lease.samples_done(&mut raw);
        assert_eq!(sources.stats().retained_bytes, 0);
    }

    #[test]
    fn simultaneous_preview_loads_share_one_sample_decode() {
        let sources = PreparedSources::new(MAX_BYTES);
        sources.select(Some("a.dng"));
        let ready = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            let load = || {
                ready.wait();
                sources.load("a.dng", fixture(), 2560).unwrap()
            };
            let a = scope.spawn(load);
            let b = scope.spawn(load);
            ready.wait();
            assert_eq!(a.join().unwrap().0, b.join().unwrap().0);
        });
        assert_eq!(sources.stats().sample_decodes, 1);
        assert_eq!(sources.stats().prepared_hits, 1);
    }
}
