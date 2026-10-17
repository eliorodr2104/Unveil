mod common;
use common::*;
use std::sync::Arc;
use unveil_ffi::*;

fn preview(s: &TestSession, frames: &Arc<Frames>, max: u32) {
    // SAFETY: s.raw is live; frames is declared before s by every caller, so it outlives the session.
    // SAFETY: the session and callback context remain alive until this frame is delivered.
    let generation = unsafe {
        uv_request_preview(
            s.raw,
            max,
            false,
            false,
            Some(record),
            Arc::as_ptr(frames) as *mut _,
        )
    };
    assert!(generation >= 1, "{}", last_error());
}

fn count(renders: &serde_json::Value, part: &str, category: &str) -> u64 {
    let rows = renders[part].as_array().unwrap();
    rows.iter().find(|c| c["category"] == category).unwrap()["count"]
        .as_u64()
        .unwrap()
}

#[test]
fn the_diagnostics_are_off_until_enabled_and_then_count_loads_and_renders() {
    let frames = Arc::new(Frames::default());
    let s = TestSession::new();
    import_fixture(&s);
    preview(&s, &frames, 256);
    wait_for(&frames, 1);

    // Off: nothing is logged or counted, but the stage sizes of library.memory are there.
    let loads = exec(&s, "unveil.loads", "{}").unwrap();
    assert!(loads["loads"].as_array().unwrap().is_empty(), "{loads}");
    let renders = exec(&s, "unveil.renders", "{}").unwrap();
    assert_eq!(count(&renders, "render", "fit-full"), 0, "{renders}");
    assert_eq!(count(&renders, "deliver", "fit-full"), 0, "{renders}");
    let memory = exec(&s, "library.memory", "{}").unwrap();
    assert!(memory["unveil"]["previewStages"]["cpu"].is_u64(), "{memory}");
    assert!(memory["unveil"]["windowStages"]["gpu"].is_u64(), "{memory}");

    // On: a new import (probe) and a render of another size (the source is cached: no decode) are seen.
    assert_eq!(
        exec(&s, "unveil.diagnostics", r#"{"enabled":true}"#).unwrap()["enabled"],
        true
    );
    let other = gradient_png(s.dir.path(), "second.png", 80, 50);
    import_png(&s, &other);
    preview(&s, &frames, 128);
    wait_for(&frames, 2);
    let loads = exec(&s, "unveil.loads", "{}").unwrap();
    let kinds: Vec<&str> = loads["loads"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"probe"), "{kinds:?}");
    assert!(
        exec(&s, "unveil.loads", "{}").unwrap()["loads"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a read clears the log"
    );
    let renders = exec(&s, "unveil.renders", r#"{"reset":true}"#).unwrap();
    assert!(count(&renders, "render", "fit-full") >= 1, "{renders}");
    assert!(count(&renders, "deliver", "fit-full") >= 1, "{renders}");
    assert_eq!(
        count(&exec(&s, "unveil.renders", "{}").unwrap(), "render", "fit-full"),
        0
    );

    // Off again: the switch is the whole cost, and turning it off empties the log.
    exec(&s, "unveil.diagnostics", r#"{"enabled":false}"#).unwrap();
    preview(&s, &frames, 64);
    wait_for(&frames, 3);
    assert_eq!(
        count(&exec(&s, "unveil.renders", "{}").unwrap(), "render", "fit-full"),
        0
    );

    // A draft has no owned view-cache image. Count its actual output path, including CPU fallback.
    exec(&s, "unveil.diagnostics", r#"{"enabled":true}"#).unwrap();
    let generation = unsafe {
        uv_request_preview(
            s.raw,
            128,
            true,
            false,
            Some(record),
            Arc::as_ptr(&frames) as *mut _,
        )
    };
    assert!(generation > 0);
    wait_for(&frames, 4);
    let renders = exec(&s, "unveil.renders", r#"{"reset":true}"#).unwrap();
    let output = renders["output"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["category"] == "fit-draft")
        .unwrap();
    let physical = output["physicalPixels"].as_u64().unwrap();
    let display = output["displayPixels"].as_u64().unwrap();
    assert!(physical > 0, "{output}");
    assert_eq!(
        physical * 4,
        display,
        "even-sized reduced output must have one quarter of the pixels: {output}"
    );
    let mapped = output["mapped"].as_u64().unwrap();
    let owned = output["owned"].as_u64().unwrap();
    assert_eq!(mapped + owned, 1, "{renders}");
    if std::env::var_os("UNVEIL_REQUIRE_GPU").is_some() {
        assert_eq!(
            mapped, 1,
            "the physical GPU path must retain its mapping: {renders}"
        );
        assert_eq!(owned, 0, "{renders}");
    }
    assert_eq!(output["mappedPixelBytes"], physical * 4 * mapped, "{renders}");
}
