mod common;
use common::*;
use unveil_ffi::*;

#[test]
fn import_select_and_set_exposure() {
    let s = TestSession::new();
    import_fixture(&s);
    exec(&s, "develop.set", r#"{"control":"light.exposure","value":0.5}"#).unwrap();
}

#[test]
fn every_basic_control_is_accepted() {
    let s = TestSession::new();
    import_fixture(&s);
    for id in [
        "light.exposure",
        "light.contrast",
        "light.highlights",
        "light.shadows",
        "light.whites",
        "light.blacks",
        "wb.temp",
        "wb.tint",
        "color.vibrance",
        "color.saturation",
    ] {
        let value = if id == "wb.temp" { 5000.0 } else { 10.0 };
        exec(
            &s,
            "develop.set",
            &format!(r#"{{"control":"{id}","value":{value}}}"#),
        )
        .unwrap_or_else(|e| panic!("{id}: {e:?}"));
    }
}

#[test]
fn unknown_command_has_its_own_status() {
    let s = TestSession::new();
    let (status, msg) = exec(&s, "no.such.command", "{}").unwrap_err();
    assert_eq!(status, UVStatus::UV_ERR_UNKNOWN_COMMAND as i32, "{msg}");
}

#[test]
fn develop_set_without_an_active_photo_is_an_engine_error() {
    let s = TestSession::new();
    let (status, msg) = exec(&s, "develop.set", r#"{"control":"light.exposure","value":0.1}"#).unwrap_err();
    assert_eq!(status, UVStatus::UV_ERR_ENGINE as i32, "{msg}");
    assert!(msg.contains("no photo selected"), "got: {msg}");
}

#[test]
fn a_file_that_is_not_an_image_is_an_engine_error_not_a_crash() {
    let s = TestSession::new();
    let bad = s.dir.path().join("broken.ARW");
    std::fs::write(&bad, b"this is not a raw file").unwrap();
    let r = exec(
        &s,
        "library.import",
        &format!(r#"{{"paths":["{}"],"mode":"add"}}"#, bad.display()),
    );
    // Either the import is refused, or it yields no photo; both must leave the session usable.
    match r {
        Ok(v) => {
            assert!(photo_ids_from_import(&v).is_empty(), "imported garbage: {v}");
            let reason = v["failed"][0][1].as_str().unwrap_or_default();
            assert!(!reason.is_empty(), "no failure reason in {v}");
        }
        Err((_, msg)) => assert!(!msg.is_empty(), "refused without a message"),
    }
    let (status, _) = exec(&s, "develop.set", r#"{"control":"light.exposure","value":0.1}"#).unwrap_err();
    assert_eq!(status, UVStatus::UV_ERR_ENGINE as i32);
}

#[test]
fn malformed_json_is_an_invalid_argument() {
    let s = TestSession::new();
    let (status, _) = exec(&s, "develop.set", "{not json").unwrap_err();
    assert_eq!(status, UVStatus::UV_ERR_INVALID_ARGUMENT as i32);
}

#[test]
fn importing_the_same_file_twice_reports_the_existing_id_as_a_duplicate() {
    let s = TestSession::new();
    let id = import_fixture(&s);
    let path = fixture_png(s.dir.path());
    let r = exec(
        &s,
        "library.import",
        &format!(r#"{{"paths":["{}"],"mode":"add"}}"#, path.display()),
    )
    .unwrap();
    assert!(photo_ids_from_import(&r).is_empty(), "imported twice: {r}");
    assert_eq!(r["duplicates"][0]["existing"].as_u64(), Some(id), "{r}");
    let existing = r["duplicates"][0]["existing"].as_u64().unwrap();
    exec(
        &s,
        "library.select",
        &format!(r#"{{"ids":[{existing}],"active":{existing}}}"#),
    )
    .unwrap();
    exec(&s, "develop.set", r#"{"control":"light.exposure","value":0.1}"#).unwrap();
}
