//! Modified for Unveil (2026-10-15): reduced drafts keep logical geometry and source selection.

use super::*;
use lightcraft_pipeline::RasterScale;

#[test]
fn reduced_draft_is_opt_in_and_keeps_source_and_logical_request() {
    let mut session = crate::Session::with_demo();
    let id = session.active().unwrap();
    let full = session.render_job(id, 4001, 3001, false, true).unwrap();
    assert_eq!(full.level, SourceLevel::Full);
    let legacy = full.clone().draft();
    assert_eq!(legacy.request.raster_scale, RasterScale::Native);
    let draft = full.clone().reduced_draft();
    assert_eq!(draft.level, full.level);
    assert_eq!(draft.source_key, full.source_key);
    assert_eq!(draft.source_epoch, full.source_epoch);
    assert_eq!((draft.request.max_w, draft.request.max_h), (4001, 3001));
    assert_eq!(draft.request.raster_scale, RasterScale::Half);
    assert_eq!(draft.request.quality, Quality::Full);
    assert_eq!(legacy.clone().reduced_draft().key, draft.key);
    assert_ne!(draft.key, full.key);
    assert_ne!(draft.key, legacy.key);
    assert_eq!(draft.clone().reduced_draft().key, draft.key);
}

#[test]
fn reduced_draft_never_uses_a_persistent_full_quality_result_cache() {
    let mut session = crate::Session::with_demo();
    let id = session.active().unwrap();
    let mut job = session.thumb_job(id, 200).unwrap();
    job.view_cache = job.cache.clone();
    let draft = job.reduced_draft();
    assert!(draft.cache.is_none());
    assert!(draft.view_cache.is_none());
}
