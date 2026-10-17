mod common;

use common::*;
use serde_json::json;

/// assert_edited checks the summary consumed by the app and both catalog filter states.
fn assert_edited(session: &TestSession, id: u64, edited: bool) {
    let summary = exec_ok(
        session,
        "catalog.query",
        &json!({"filter": {"only": [id]}}).to_string(),
    );
    assert_eq!(summary["total"], 1);
    assert_eq!(summary["photos"][0]["kind"], "image");
    assert_eq!(summary["photos"][0]["edited"], edited);
    for filter in [false, true] {
        let result = exec_ok(
            session,
            "catalog.query",
            &json!({"filter": {"only": [id], "edited": filter}}).to_string(),
        );
        assert_eq!(result["total"], usize::from(filter == edited));
    }
}

#[test]
fn jpeg_white_balance_edits_reach_the_catalog_and_follow_undo_redo_reset() {
    let session = TestSession::new();
    let path = session.dir.path().join("white-balance.jpg");
    let pixels: Vec<u8> = (0..32)
        .flat_map(|y| (0..48).flat_map(move |x| [(x * 5) as u8, (y * 7) as u8, 120]))
        .collect();
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, 90)
        .encode(&pixels, 48, 32, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    std::fs::write(&path, jpeg).unwrap();
    let imported = exec_ok(
        &session,
        "library.import",
        &json!({"paths": [path], "mode": "add"}).to_string(),
    );
    let id = photo_id_from_import(&imported);
    exec_ok(
        &session,
        "library.select",
        &json!({"ids": [id], "active": id}).to_string(),
    );
    assert_edited(&session, id, false);

    for (control, value) in [("wb.temp", 8000.0), ("wb.tint", 25.0)] {
        exec_ok(
            &session,
            "develop.set",
            &json!({"control": control, "value": value}).to_string(),
        );
        assert_edited(&session, id, true);
        exec_ok(&session, "edit.undo", "{}");
        assert_edited(&session, id, false);
        exec_ok(&session, "edit.redo", "{}");
        assert_edited(&session, id, true);
        exec_ok(&session, "develop.wb", r#"{"mode":"asShot"}"#);
        assert_edited(&session, id, false);
    }

    exec_ok(&session, "develop.wb", r#"{"mode":"daylight"}"#);
    assert_edited(&session, id, true);
    exec_ok(&session, "develop.reset", "{}");
    assert_edited(&session, id, false);
}
