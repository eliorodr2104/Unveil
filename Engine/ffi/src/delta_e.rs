//! delta_e measures how far apart two renders are, in CIEDE2000. It is a Rust-only helper for the
//! golden comparison: nothing here is exported through the C ABI.

/// D65 reference white (CIE 1931 2 degrees), the one sRGB uses.
const WHITE: [f64; 3] = [0.95047, 1.0, 1.08883];

/// srgb8_to_lab converts an 8-bit sRGB triple to CIELAB under D65.
pub fn srgb8_to_lab(rgb: [u8; 3]) -> [f64; 3] {
    let lin = rgb.map(|c| {
        let c = f64::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    let [r, g, b] = lin;
    let xyz = [
        0.4124564 * r + 0.3575761 * g + 0.1804375 * b,
        0.2126729 * r + 0.7151522 * g + 0.0721750 * b,
        0.0193339 * r + 0.1191920 * g + 0.9503041 * b,
    ];
    let f = |t: f64| {
        const E: f64 = 216.0 / 24389.0;
        const K: f64 = 24389.0 / 27.0;
        if t > E { t.cbrt() } else { (K * t + 16.0) / 116.0 }
    };
    let [fx, fy, fz] = [f(xyz[0] / WHITE[0]), f(xyz[1] / WHITE[1]), f(xyz[2] / WHITE[2])];
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// ciede2000 is the CIEDE2000 colour difference of two CIELAB colours, with kL = kC = kH = 1.
/// It follows Sharma, Wu and Dalal (2005), including the hue-angle corner cases.
pub fn ciede2000(lab1: [f64; 3], lab2: [f64; 3]) -> f64 {
    let [l1, a1, b1] = lab1;
    let [l2, a2, b2] = lab2;
    let c_bar = (a1.hypot(b1) + a2.hypot(b2)) / 2.0;
    let g = 0.5 * (1.0 - (c_bar.powi(7) / (c_bar.powi(7) + 25f64.powi(7))).sqrt());
    let (a1p, a2p) = ((1.0 + g) * a1, (1.0 + g) * a2);
    let (c1p, c2p) = (a1p.hypot(b1), a2p.hypot(b2));
    let hue = |b: f64, a: f64| {
        if b == 0.0 && a == 0.0 {
            0.0
        } else {
            b.atan2(a).to_degrees().rem_euclid(360.0)
        }
    };
    let (h1p, h2p) = (hue(b1, a1p), hue(b2, a2p));

    let dlp = l2 - l1;
    let dcp = c2p - c1p;
    let dhp = if c1p * c2p == 0.0 {
        0.0
    } else if (h2p - h1p).abs() <= 180.0 {
        h2p - h1p
    } else if h2p - h1p > 180.0 {
        h2p - h1p - 360.0
    } else {
        h2p - h1p + 360.0
    };
    let dhp_term = 2.0 * (c1p * c2p).sqrt() * (dhp / 2.0).to_radians().sin();

    let lp_bar = (l1 + l2) / 2.0;
    let cp_bar = (c1p + c2p) / 2.0;
    let hp_bar = if c1p * c2p == 0.0 {
        h1p + h2p
    } else if (h1p - h2p).abs() <= 180.0 {
        (h1p + h2p) / 2.0
    } else if h1p + h2p < 360.0 {
        (h1p + h2p + 360.0) / 2.0
    } else {
        (h1p + h2p - 360.0) / 2.0
    };
    let t = 1.0 - 0.17 * (hp_bar - 30.0).to_radians().cos()
        + 0.24 * (2.0 * hp_bar).to_radians().cos()
        + 0.32 * (3.0 * hp_bar + 6.0).to_radians().cos()
        - 0.20 * (4.0 * hp_bar - 63.0).to_radians().cos();
    let d_theta = 30.0 * (-((hp_bar - 275.0) / 25.0).powi(2)).exp();
    let rc = 2.0 * (cp_bar.powi(7) / (cp_bar.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lp_bar - 50.0).powi(2) / (20.0 + (lp_bar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cp_bar;
    let sh = 1.0 + 0.015 * cp_bar * t;
    let rt = -(2.0 * d_theta).to_radians().sin() * rc;

    let (xl, xc, xh) = (dlp / sl, dcp / sc, dhp_term / sh);
    (xl * xl + xc * xc + xh * xh + rt * xc * xh).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharma_reference_pairs() {
        let pairs = [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 3.1571, -77.2803], [50.0, 0.0, -82.7485], 2.8615),
            ([50.0, 2.8361, -74.0200], [50.0, 0.0, -82.7485], 3.4412),
        ];
        for (a, b, expected) in pairs {
            let d = ciede2000(a, b);
            assert!((d - expected).abs() < 1e-4, "{a:?} vs {b:?}: {d} != {expected}");
        }
    }

    /// These pairs sit near the hue-angle seam and across the neutral axis, so they reach the
    /// delta-h and mean-hue wraps that the three pairs above (hue near 270) never touch.
    #[test]
    fn sharma_hue_wrap_pairs() {
        let pairs = [
            ([50.0, 0.0, 0.0], [50.0, -1.0, 2.0], 2.3669),
            ([50.0, 2.5, 0.0], [50.0, 0.0, -2.5], 4.3065),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
            ([50.0, 2.5, 0.0], [61.0, -5.0, 29.0], 22.8977),
            ([50.0, 2.5, 0.0], [56.0, -27.0, -3.0], 31.9030),
        ];
        for (a, b, expected) in pairs {
            let (d, back) = (ciede2000(a, b), ciede2000(b, a));
            assert!((d - expected).abs() < 1e-4, "{a:?} vs {b:?}: {d} != {expected}");
            assert!(
                (back - expected).abs() < 1e-4,
                "{b:?} vs {a:?}: {back} != {expected}"
            );
        }
    }

    /// Primaries pin the matrix, and mid grey pins the transfer curve: 0 and 1 are fixed points of it.
    #[test]
    fn srgb_primaries_match_reference_lab() {
        let refs = [
            ([255, 0, 0], [53.2408, 80.0925, 67.2032]),
            ([0, 255, 0], [87.7347, -86.1827, 83.1793]),
            ([0, 0, 255], [32.2970, 79.1875, -107.8602]),
            ([128, 128, 128], [53.5850, 0.0, 0.0]),
        ];
        for (rgb, expected) in refs {
            let lab = srgb8_to_lab(rgb);
            for (got, want) in lab.iter().zip(expected) {
                assert!((got - want).abs() < 2e-3, "{rgb:?}: {lab:?} != {expected:?}");
            }
        }
    }

    #[test]
    fn identical_colors_have_zero_distance() {
        let lab = srgb8_to_lab([120, 30, 200]);
        assert_eq!(ciede2000(lab, lab), 0.0);
    }

    #[test]
    fn srgb_white_is_l100() {
        let lab = srgb8_to_lab([255, 255, 255]);
        assert!(
            (lab[0] - 100.0).abs() < 0.01 && lab[1].abs() < 0.01 && lab[2].abs() < 0.01,
            "{lab:?}"
        );
    }
}
