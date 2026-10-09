#![allow(dead_code)]

use std::ffi::{CStr, CString};
use std::path::{Path, PathBuf};

use unveil_ffi::*;

/// TestSession owns a UVSession for one test and frees it on drop.
pub struct TestSession {
    pub raw: *mut UVSession,
    pub dir: tempdir_lite::Dir,
}

impl TestSession {
    pub fn new() -> TestSession {
        let dir = tempdir_lite::Dir::new();
        let path = CString::new(dir.path().to_str().unwrap()).unwrap();
        // SAFETY: path is a NUL-terminated UTF-8 string that outlives the call.
        let raw = unsafe { uv_session_new(path.as_ptr(), 512 << 20) };
        assert!(!raw.is_null(), "uv_session_new failed: {}", last_error());
        TestSession { raw, dir }
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        // SAFETY: raw came from uv_session_new and is freed only here, once.
        unsafe { uv_session_free(self.raw) };
    }
}

pub fn last_error() -> String {
    let p = uv_last_error();
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: p is non-NULL and valid until the next uv_* call on this thread; copied at once.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

/// fixture_png writes a 96x64 RGB gradient the engine can import without any RAW file.
pub fn fixture_png(dir: &Path) -> PathBuf {
    gradient_png(dir, "fixture.png", 96, 64)
}

/// gradient_png writes a width x height RGB gradient named `name` into dir. Different sizes give
/// different content, so the engine never sees two of them as duplicates.
pub fn gradient_png(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
    let path = dir.join(name);
    let file = std::fs::File::create(&path).unwrap();
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgb);
    let mut w = enc.write_header().unwrap();
    let data: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).flat_map(move |x| [(x * 2) as u8, (y * 3) as u8, 128]))
        .collect();
    w.write_image_data(&data).unwrap();
    path
}

/// exec runs one command through uv_execute: the parsed result, or the status and uv_last_error.
pub fn exec(s: &TestSession, cmd: &str, params: &str) -> Result<serde_json::Value, (i32, String)> {
    let c = CString::new(cmd).unwrap();
    let p = CString::new(params).unwrap();
    let mut out = std::ptr::null_mut();
    // SAFETY: s.raw is a live session; c, p and out outlive the call.
    let status = unsafe { uv_execute(s.raw, c.as_ptr(), p.as_ptr(), &mut out) };
    if status != 0 {
        return Err((status, last_error()));
    }
    // SAFETY: on UV_OK, out is a NUL-terminated string owned by us until uv_string_free.
    let json = unsafe { CStr::from_ptr(out) }.to_str().unwrap().to_owned();
    // SAFETY: out came from uv_execute and is freed once, after the copy above.
    unsafe { uv_string_free(out) };
    Ok(serde_json::from_str(&json).unwrap_or(serde_json::Value::Null))
}

/// exec_ok is exec for commands that must succeed; it panics with the status and message.
pub fn exec_ok(s: &TestSession, cmd: &str, params: &str) -> serde_json::Value {
    exec(s, cmd, params).unwrap_or_else(|e| panic!("{cmd} {params}: {e:?}"))
}

/// import_png imports the image at path and makes it the active photo; returns its id.
pub fn import_png(s: &TestSession, path: &Path) -> u64 {
    let r = exec_ok(
        s,
        "library.import",
        &format!(r#"{{"paths":["{}"],"mode":"add"}}"#, path.display()),
    );
    let id = photo_id_from_import(&r);
    exec_ok(s, "library.select", &format!(r#"{{"ids":[{id}],"active":{id}}}"#));
    id
}

/// import_fixture imports the gradient PNG and makes it the active photo; returns its id.
pub fn import_fixture(s: &TestSession) -> u64 {
    import_png(s, &fixture_png(s.dir.path()))
}

/// tempdir_lite avoids a tempfile dependency: a unique folder under the system temp dir.
pub mod tempdir_lite {
    pub struct Dir(std::path::PathBuf);
    impl Dir {
        pub fn new() -> Dir {
            let n = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            // The clock ticks in microseconds on macOS, so parallel tests also need a counter.
            static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let seq = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!("unveil-ffi-{}-{n}-{seq}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Dir(p)
        }
        pub fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// photo_ids_from_import reads the ids of `library.import`'s `imported` array.
pub fn photo_ids_from_import(result: &serde_json::Value) -> Vec<u64> {
    result["imported"]
        .as_array()
        .unwrap_or_else(|| panic!("no `imported` array in {result}"))
        .iter()
        .map(|id| {
            id.as_u64()
                .unwrap_or_else(|| panic!("non-integer photo id in {result}"))
        })
        .collect()
}

/// photo_id_from_import is the first imported id; it panics, showing the JSON, when none was imported.
pub fn photo_id_from_import(result: &serde_json::Value) -> u64 {
    photo_ids_from_import(result)
        .first()
        .copied()
        .unwrap_or_else(|| panic!("nothing imported: {result}"))
}
