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
        let raw = unsafe { uv_session_new(path.as_ptr(), 512 << 20) };
        assert!(!raw.is_null(), "uv_session_new failed: {}", last_error());
        TestSession { raw, dir }
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        unsafe { uv_session_free(self.raw) };
    }
}

pub fn last_error() -> String {
    let p = uv_last_error();
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

/// fixture_png writes a 96x64 RGB gradient the engine can import without any RAW file.
pub fn fixture_png(dir: &Path) -> PathBuf {
    let path = dir.join("fixture.png");
    let file = std::fs::File::create(&path).unwrap();
    let mut enc = png::Encoder::new(file, 96, 64);
    enc.set_color(png::ColorType::Rgb);
    let mut w = enc.write_header().unwrap();
    let data: Vec<u8> = (0..64)
        .flat_map(|y| (0..96).flat_map(move |x| [x as u8 * 2, y as u8 * 3, 128]))
        .collect();
    w.write_image_data(&data).unwrap();
    path
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
            let p = std::env::temp_dir().join(format!("unveil-ffi-{}-{n}", std::process::id()));
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
