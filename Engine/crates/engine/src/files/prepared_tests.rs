//! Modified for Unveil (2026-10-14): sensor reuse, exact output, invalidation and fixture decode-count gates.
use super::*;

fn fixture(seed: u16) -> Vec<u8> {
    crate::tests_xmp::synthetic_dng_of(32, 24, "RGGB", vec![seed; 32 * 24], None, Default::default())
}

#[test]
fn first_full_development_reuses_preview_samples_and_releases_them() {
    let bytes = fixture(4096);
    let sources = PreparedSources::new(128 << 20);
    let token = sources.begin_open("a.dng");
    assert!(sources.confirm_open(token));
    let preview = sources.load("a.dng", bytes.clone(), 2560).unwrap();
    assert_eq!(preview.0, load_bytes(&bytes, 2560).unwrap().0);
    assert_eq!(sources.stats().sample_decodes, 1);
    assert!(sources.stats().retained_bytes >= 32 * 24 * 2);
    let full = sources.load("a.dng", bytes.clone(), usize::MAX).unwrap();
    assert_eq!(full.0, load_bytes(&bytes, usize::MAX).unwrap().0);
    assert_eq!(sources.stats().sample_decodes, 1, "first zoom must not invoke the RAW sample decoder again");
    assert_eq!(sources.stats().prepared_hits, 1);
    assert_eq!(sources.stats().retained_bytes, 0);
}

#[test]
fn changed_content_at_the_same_path_cannot_reuse_old_samples() {
    let sources = PreparedSources::new(128 << 20);
    let token = sources.begin_open("same.dng");
    assert!(sources.confirm_open(token));
    sources.load("same.dng", fixture(4096), 2560).unwrap();
    let changed = fixture(8192);
    let full = sources.load("same.dng", changed.clone(), usize::MAX).unwrap();
    assert_eq!(full.0, load_bytes(&changed, usize::MAX).unwrap().0);
    assert_eq!(sources.stats().sample_decodes, 2);
    assert_eq!(sources.stats().prepared_hits, 0);
    assert_eq!(sources.stats().content_misses, 1);
}

#[test]
fn oversized_samples_and_pressure_fall_back_without_retention() {
    for pressure in [false, true] {
        let sources = PreparedSources::new(if pressure { 128 << 20 } else { 1 });
        let token = sources.begin_open("a.dng");
        assert!(sources.confirm_open(token));
        sources.load("a.dng", fixture(4096), 2560).unwrap();
        if pressure {
            sources.clear();
        }
        assert_eq!(sources.stats().retained_bytes, 0);
        sources.load("a.dng", fixture(4096), usize::MAX).unwrap();
        assert_eq!(sources.stats().sample_decodes, 2);
        assert_eq!(sources.stats().prepared_hits, 0);
        assert_eq!(sources.stats().retained_bytes, 0);
    }
}

#[test]
fn replacement_cancels_the_previous_open_and_does_not_cache_thumbnails() {
    let sources = PreparedSources::new(128 << 20);
    let old = sources.begin_open("a.dng");
    let current = sources.begin_open("b.dng");
    assert!(!sources.confirm_open(old));
    assert!(sources.confirm_open(current));
    sources.load("a.dng", fixture(4096), 2560).unwrap();
    sources.load("b.dng", fixture(4096), 512).unwrap();
    assert_eq!(sources.stats().retained_bytes, 0);
    sources.load("b.dng", fixture(4096), 2560).unwrap();
    assert!(sources.stats().retained_bytes > 0);
}

#[test]
fn missing_denoise_product_keeps_the_plain_source_and_reuses_preparation() {
    let sources = PreparedSources::new(128 << 20);
    sources.select(Some("a.dng"));
    let bytes = fixture(4096);
    sources.load("a.dng", bytes.clone(), 2560).unwrap();
    let spec = DenoiseSpec { product: "missing-e3-denoise-product".into(), key: "e3".into(), make: None };
    let result = sources.load_pair("a.dng", bytes.clone(), usize::MAX, &spec).unwrap();
    assert_eq!(result.0, load_bytes(&bytes, usize::MAX).unwrap().0);
    assert!(result.1.is_none());
    assert_eq!(sources.stats().sample_decodes, 1);
    assert_eq!(sources.stats().retained_bytes, 0);
}

#[test]
fn prepared_pair_preserves_the_denoised_product_and_plain_pixels() {
    let dir = crate::tests_xmp::temp_dir("prepared-denoise");
    let bytes = fixture(4096);
    let raw = lightcraft_raw::decode(&bytes).unwrap();
    let product = raw.develop(lightcraft_raw::Method::Ahd).unwrap();
    let path = dir.join("test.lcdn");
    lightcraft_denoise::product::write(&path, &product, "prepared-denoise").unwrap();
    let spec = DenoiseSpec { product: path, key: "prepared-denoise".into(), make: None };
    let sources = PreparedSources::new(128 << 20);
    sources.select(Some("a.dng"));
    sources.load("a.dng", bytes.clone(), 2560).unwrap();
    let actual = sources.load_pair("a.dng", bytes.clone(), usize::MAX, &spec).unwrap();
    let expected = load_vec_pair(bytes, usize::MAX, &spec).unwrap();
    assert_eq!(actual.0, expected.0);
    assert_eq!(actual.1, expected.1);
    assert!(actual.1.is_some());
    assert_eq!(sources.stats().sample_decodes, 1);
    assert_eq!(sources.stats().retained_bytes, 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
#[ignore = "requires the E0 RAF and NEF fixtures; set UNVEIL_E3_RAF and UNVEIL_E3_NEF"]
fn e0_open_and_first_zoom_decode_each_fixture_once() {
    for variable in ["UNVEIL_E3_RAF", "UNVEIL_E3_NEF"] {
        let path = std::env::var(variable).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let sources = PreparedSources::new(128 << 20);
        let token = sources.begin_open(&path);
        #[cfg(feature = "decode-counts")]
        let before_probe = lightcraft_raw::decode_counts();
        let probed = sources.probe(&path, &bytes).unwrap();
        #[cfg(feature = "decode-counts")]
        let probe_calls = lightcraft_raw::decode_counts().sample_dispatches - before_probe.sample_dispatches;
        assert!(sources.confirm_open(token));
        assert_eq!(probed.width, probe_bytes(&path, &bytes).unwrap().width);
        #[cfg(feature = "decode-counts")]
        let before_preview = lightcraft_raw::decode_counts();
        let preview = sources.load(&path, bytes.clone(), 2560).unwrap();
        #[cfg(feature = "decode-counts")]
        let preview_calls = lightcraft_raw::decode_counts().sample_dispatches - before_preview.sample_dispatches;
        assert_eq!(preview.0, load_bytes(&bytes, 2560).unwrap().0);
        let retained = sources.stats().retained_bytes;
        assert!(retained > 0 && retained <= 128 << 20);
        #[cfg(feature = "decode-counts")]
        let before_full = lightcraft_raw::decode_counts();
        let full = sources.load(&path, bytes.clone(), usize::MAX).unwrap();
        #[cfg(feature = "decode-counts")]
        {
            let full_calls = lightcraft_raw::decode_counts().sample_dispatches - before_full.sample_dispatches;
            assert_eq!(full_calls, 0, "{variable}: first full must bypass the actual sample decoder dispatch");
            assert_eq!(probe_calls + preview_calls + full_calls, 1);
            eprintln!("{variable}: actual sample dispatches probe={probe_calls}, preview={preview_calls}, first-full={full_calls}");
        }
        assert_eq!(full.0, load_bytes(&bytes, usize::MAX).unwrap().0);
        let stats = sources.stats();
        assert_eq!(stats.sample_decodes, 1, "{variable}: probe -> preview -> first full");
        assert_eq!(stats.retained_bytes, 0);
        eprintln!("{variable}: sample decodes {}, prepared hits {}, fit retained {retained} bytes", stats.sample_decodes, stats.prepared_hits);
    }
}

#[test]
#[ignore = "requires the five baseline RAWs; set UNVEIL_RAW_DIR"]
fn five_raw_prepared_outputs_match_ordinary_development() {
    let dir = std::path::PathBuf::from(std::env::var_os("UNVEIL_RAW_DIR").unwrap());
    for name in [
        "DSCF0267.RAF",
        "Z7-14bit-lossless-L.NEF",
        "Canon_EOS_R5_RAW_ISO_100_nocrop_nodual.CR3",
        "iPhone12Pro_IMG_1361.DNG",
        "ILCE-7M4_DSC06674_LossLess-Large.ARW",
    ] {
        let path = dir.join(name);
        let path = path.to_str().unwrap();
        let bytes = std::fs::read(path).unwrap();
        let sources = PreparedSources::new(128 << 20);
        sources.begin_open(path);
        sources.probe(path, &bytes).unwrap();
        sources.select(Some(path));
        for edge in [2560, usize::MAX] {
            let actual = sources.load(path, bytes.clone(), edge).unwrap();
            let expected = load_bytes(&bytes, edge).unwrap();
            assert_eq!(actual.0, expected.0, "{name}, edge {edge}");
            assert!(sources.stats().retained_bytes <= 128 << 20);
        }
        assert_eq!(sources.stats().retained_bytes, 0);
        eprintln!("{name}: preview and full output match exactly");
    }
}
