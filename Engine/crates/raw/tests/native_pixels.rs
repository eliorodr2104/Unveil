//! Modified for Unveil (2026-10-17): full-resolution pixel fingerprints for the unchanged RAW corpus.
use sha2::{Digest, Sha256};

#[test]
#[ignore = "requires UNVEIL_RAW_DIR with the five photographic fixtures"]
fn full_resolution_camera_pixels() {
    let root = std::path::PathBuf::from(std::env::var_os("UNVEIL_RAW_DIR").expect("UNVEIL_RAW_DIR is required"));
    for name in [
        "DSCF0267.RAF",
        "Z7-14bit-lossless-L.NEF",
        "ILCE-7M4_DSC06674_LossLess-Large.ARW",
        "Canon_EOS_R5_RAW_ISO_100_nocrop_nodual.CR3",
        "iPhone12Pro_IMG_1361.DNG",
    ] {
        let bytes = std::fs::read(root.join(name)).unwrap();
        let raw = lightcraft_raw::decode(&bytes).unwrap();
        drop(bytes);
        let image = raw.develop(lightcraft_raw::Method::Ahd).unwrap().into_oriented(raw.orientation);
        drop(raw);
        let mut digest = Sha256::new();
        let mut block = Vec::with_capacity(1024 * 12);
        for pixels in image.data.chunks(1024) {
            block.clear();
            for value in pixels.iter().flatten() {
                block.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            digest.update(&block);
        }
        println!("RAW-PIXELS {name} {} {} {:x}", image.width, image.height, digest.finalize());
    }
}
