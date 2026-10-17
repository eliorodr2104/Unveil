//! Identity follows one decoded source through host and device residency.

use std::sync::atomic::{AtomicU64, Ordering};

/// An opaque identity for one source interpretation in this process. Reconstructing the same
/// source preserves it; replacement, reload or a new decoded generation gets a new identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceId(u64);

impl SourceId {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for SourceId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SourceExtent {
    pub width: usize,
    pub height: usize,
}

impl From<&lightcraft_raster::Rgb32f> for SourceExtent {
    fn from(image: &lightcraft_raster::Rgb32f) -> Self {
        Self { width: image.width, height: image.height }
    }
}
