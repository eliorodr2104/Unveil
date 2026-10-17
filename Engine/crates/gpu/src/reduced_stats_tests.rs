// Modified for Unveil (2026-10-17): memory ownership and reserve policy; see Engine/CHANGES.md.
use super::*;
use crate::reduced_stats::{ReducedStats, WORDS};
use lightcraft_raster::Rgba8;

fn check(image: &Rgba8, crop: [usize; 4], omitted: bool) {
    let gpu = crate::device().expect("real GPU required");
    let _render = crate::ctx::RenderScope::new(gpu);
    let errors = crate::ctx::ErrorScopes::push(gpu);
    let mut cx = Cx::new(gpu);
    let [x, y, sw, sh] = crop;
    let (dw, dh) = (sw.div_ceil(2), sh.div_ceil(2));
    let count = dw * dh;
    let input = gpu.upload(image.data.as_flattened());
    let reference = cx.zeroed(count);
    let output = cx.zeroed(count + 1 + WORDS);
    let stats = ReducedStats::new(&mut cx, dw, dh);
    let params = [image.width, image.height, x, y, sw, sh, dw, dh, image.data.len(), stats.step, crate::reduced_stats::TILE].map(|v| v as u32);
    let groups = groups2(dw, dh, [16, 16]);
    cx.run("display_reduce_checked_fast", &params, &[Some(&input), Some(&reference), None], groups);
    cx.run(
        "display_reduce_checked_stats",
        &params,
        &[Some(&input), Some(&output), Some(&stats.buffer)],
        if omitted { [1, 1, 1] } else { groups2(dw, dh, [16 * crate::reduced_stats::TILE, 16 * crate::reduced_stats::TILE]) },
    );
    cx.copy_into(&stats.buffer, &output, count + 1);
    let readback = cx.map(&output, count + 1 + WORDS, 0).unwrap();
    let (pixels, decoded, words) = readback
        .with_bytes(|bytes| {
            assert_eq!(&bytes[count * 4..count * 4 + 4], &[0; 4], "precision word stays separate");
            let pixels = bytemuck::cast_slice::<u8, [u8; 4]>(&bytes[..count * 4]).to_vec();
            let summary = &bytes[(count + 1) * 4..];
            (pixels, stats.decode(summary), bytemuck::cast_slice::<u8, u32>(summary).to_vec())
        })
        .unwrap();
    if omitted {
        assert!(decoded.is_err(), "partial dispatch must not become a complete histogram");
    } else {
        let expected: Vec<[u8; 4]> = cx.read(&reference, count);
        assert_eq!(pixels, expected, "statistics changed reduced pixels for {crop:?}");
        let image = Rgba8 { width: dw, height: dh, data: lightcraft_raster::Buffer::Heap(pixels) };
        let (missing, histogram) = decoded.unwrap();
        let cpu = Histogram::of_srgb8(&image);
        assert_eq!(histogram, cpu, "all bins and total for crop {crop:?}, step {}", stats.step);
        assert_eq!(histogram.clipping(), cpu.clipping());
        assert_eq!(missing, image.data.iter().filter(|p| p[3] != 255).count());
        let mut corrupt = words.clone();
        corrupt[1025] -= 1;
        assert!(stats.decode(bytemuck::cast_slice(&corrupt)).is_err());
        corrupt = words.clone();
        corrupt[1024] = count as u32 + 1;
        assert!(stats.decode(bytemuck::cast_slice(&corrupt)).is_err());
        corrupt = words;
        corrupt[0] += 1;
        assert!(stats.decode(bytemuck::cast_slice(&corrupt)).is_err());
        assert!(stats.decode(&[0; 8]).is_err());
    }
    assert_eq!(errors.pop().map(|e| e.reason), None);
}

#[test]
fn fused_reduced_histogram_is_exact_for_crops_odd_sizes_and_sample_strides() {
    for (w, h) in [(1, 1), (17, 13), (33, 34), (2360, 1573), (2049, 2049), (4001, 4001)] {
        let image = Rgba8::from_fn(w, h, |x, y| [(x * 13 + y) as u8, (y * 17) as u8, (x ^ y) as u8, 255]);
        check(&image, [0, 0, w, h], false);
        if w > 33 {
            check(&image, [7, 3, w - 13, h - 11], false);
        }
    }
    for value in [0, 255] {
        check(&Rgba8::from_fn(67, 43, |_, _| [value, value, value, 255]), [3, 1, 61, 41], false);
    }
}

#[test]
fn fused_reduced_histogram_checks_unsampled_alpha_and_rejects_missing_work() {
    let mut image = Rgba8::from_fn(2049, 2049, |x, y| [x as u8, y as u8, 128, 255]);
    image.data[3 * image.width + 3][3] = 0;
    image.data[0][3] = 12;
    let last = image.data.len() - 1;
    image.data[last][3] = 254;
    check(&image, [0, 0, image.width, image.height], false);
    // Only positive-area contributors to the visible crop are part of the display contract.
    check(&image, [9, 11, 31, 29], false);
    check(&image, [0, 0, 513, 501], true);
}
