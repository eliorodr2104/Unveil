// Modified by the Unveil authors, 2026: sample borrowed RGBA views without an owned image; see Engine/CHANGES.md.
//! Histograms of display-encoded images (what the Histogram panel shows).

use serde::Serialize;

use crate::Rgba8;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Histogram {
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
    pub luma: Vec<u32>,
    pub total: u32,
}

impl Histogram {
    pub const BINS: usize = 256;

    pub fn of_srgb8(img: &Rgba8) -> Histogram {
        Self::of_srgb8_view(img.width, img.height, |x, y| img.get(x, y))
    }

    /// Sample a borrowed image or region without allocating an intermediate raster. `pixel` is
    /// called only within the supplied dimensions; its origin is the region's own top-left corner.
    pub fn of_srgb8_view(width: usize, height: usize, pixel: impl Fn(usize, usize) -> [u8; 4]) -> Histogram {
        let mut h = Histogram { r: vec![0; 256], g: vec![0; 256], b: vec![0; 256], luma: vec![0; 256], total: 0 };
        // Sample at most ~1 MP for speed.
        let step = ((width as f64 * height as f64 / 1_000_000.0).sqrt().ceil() as usize).max(1);
        for y in (0..height).step_by(step) {
            for x in (0..width).step_by(step) {
                let p = pixel(x, y);
                h.r[p[0] as usize] += 1;
                h.g[p[1] as usize] += 1;
                h.b[p[2] as usize] += 1;
                let l = (p[0] as u32 * 54 + p[1] as u32 * 183 + p[2] as u32 * 19) >> 8;
                h.luma[l.min(255) as usize] += 1;
                h.total += 1;
            }
        }
        h
    }

    /// Fraction of pixels clipped at black / white in any channel (approximate: per channel max).
    pub fn clipping(&self) -> (f32, f32) {
        if self.total == 0 {
            return (0.0, 0.0);
        }
        let t = self.total as f32;
        let lo = self.r[0].max(self.g[0]).max(self.b[0]) as f32 / t;
        let hi = self.r[255].max(self.g[255]).max(self.b[255]) as f32 / t;
        (lo, hi)
    }

    /// Mean luma in 0..1.
    pub fn mean_luma(&self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        self.luma.iter().enumerate().map(|(i, &c)| i as f32 * c as f32).sum::<f32>() / (255.0 * self.total as f32)
    }

    /// Luma value (0..1) below which `q` of the pixels fall.
    pub fn percentile(&self, q: f32) -> f32 {
        let target = (q.clamp(0.0, 1.0) * self.total as f32) as u32;
        let mut acc = 0;
        for (i, &c) in self.luma.iter().enumerate() {
            acc += c;
            if acc >= target {
                return i as f32 / 255.0;
            }
        }
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_stats() {
        let img = Rgba8::from_fn(10, 10, |x, _| if x < 5 { [0, 0, 0, 255] } else { [255, 255, 255, 255] });
        let h = Histogram::of_srgb8(&img);
        assert_eq!(h.total, 100);
        assert_eq!(h.r[0], 50);
        assert_eq!(h.luma[255], 50);
        let (lo, hi) = h.clipping();
        assert!((lo - 0.5).abs() < 1e-6 && (hi - 0.5).abs() < 1e-6);
        assert!((h.mean_luma() - 0.5).abs() < 1e-3);
        assert_eq!(h.percentile(0.25), 0.0);
        assert_eq!(h.percentile(0.75), 1.0);
    }

    #[test]
    fn borrowed_region_matches_owned_crop_and_sampling_grid() {
        let source = Rgba8::from_fn(2049, 1537, |x, y| [(x * 13 + y) as u8, (y * 17) as u8, (x ^ y) as u8, 255]);
        for (x, y, w, h) in [(3, 5, 17, 11), (1, 1, 2048, 1536), (37, 19, 1001, 1001)] {
            let expected = Histogram::of_srgb8(&source.crop(x, y, w, h));
            let actual = Histogram::of_srgb8_view(w, h, |px, py| source.get(x + px, y + py));
            assert_eq!(actual, expected, "region {x},{y} {w}x{h}");
        }
    }

    #[test]
    fn empty_borrowed_view_never_reads_a_pixel() {
        for (w, h) in [(0, 0), (0, 17), (19, 0)] {
            let actual = Histogram::of_srgb8_view(w, h, |_, _| panic!("empty view sampled"));
            assert_eq!(actual, Histogram::of_srgb8(&Rgba8::new(w, h)));
        }
    }
}
