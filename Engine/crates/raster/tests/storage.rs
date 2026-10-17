// Modified for Unveil (2026-10-17): memory ownership and reserve policy; see Engine/CHANGES.md.
use lightcraft_raster::{Buffer, Rgb32f};

#[test]
fn returnable_pixels_survive_clone_crop_and_orientation() {
    let mut image = Rgb32f::new_returnable(600, 600);
    for (i, p) in image.data.iter_mut().enumerate() {
        *p = [i as f32, -0.0, 3.25];
    }
    let expected = image.clone();
    let shared = std::sync::Arc::new(image);
    let survivor = shared.clone();
    drop(shared);
    assert_eq!(*survivor, expected);
    let cropped = std::sync::Arc::unwrap_or_clone(survivor).into_crop(3, 4, 200, 201);
    assert_eq!(cropped, expected.crop(3, 4, 200, 201));
    assert_eq!(cropped.rotate_cw().rotate_cw(), cropped.rotate_180());
}

#[test]
fn adopting_an_existing_vector_does_not_copy_its_pixels() {
    let original = vec![3u32; 100];
    let pointer = original.as_ptr();
    let data = Buffer::from(original);
    assert_eq!(data.as_ptr(), pointer);
    assert_eq!(&*data, &[3; 100]);
}
