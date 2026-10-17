//! Prepared probes preserve sample work when the vendor needs it to describe the image.
//! Modified for Unveil (2026-10-14): validate the E0 NEF and RAF probe handoff.

use lightcraft_raw::{RawFormat, decode, probe_with_samples};

#[test]
#[ignore = "requires the E0 NEF fixture; set UNVEIL_E3_NEF"]
fn sample_reading_probe_keeps_the_exact_nef_image() {
    let path = std::env::var_os("UNVEIL_E3_NEF").expect("set UNVEIL_E3_NEF to the E0 NEF fixture");
    let bytes = std::fs::read(path).unwrap();
    let (info, prepared) = probe_with_samples(&bytes).unwrap();
    let prepared = prepared.expect("the NEF probe already decoded samples");
    assert_eq!(info.format, RawFormat::Nef);
    assert_eq!(info, prepared.info());
    assert_eq!(prepared, decode(&bytes).unwrap());
}

#[test]
#[ignore = "requires the E0 RAF fixture; set UNVEIL_E3_RAF"]
fn header_only_probe_does_not_start_a_raf_sample_decode() {
    let path = std::env::var_os("UNVEIL_E3_RAF").expect("set UNVEIL_E3_RAF to the E0 RAF fixture");
    let bytes = std::fs::read(path).unwrap();
    let (info, prepared) = probe_with_samples(&bytes).unwrap();
    assert_eq!(info.format, RawFormat::Raf);
    assert!(prepared.is_none());
    assert_eq!(info, decode(&bytes).unwrap().info());
}
