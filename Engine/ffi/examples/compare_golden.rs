//! compare_golden <mac_dir> <ipad_dir>: CIEDE2000 between the PNGs of the two directories, as a Markdown
//! table. Exit 1 if a pair is missing, differs in size, or has mean > 0.5 or p99 > 2.0.
use std::path::Path;
use unveil_ffi::delta_e::{ciede2000, srgb8_to_lab};

const MAX_MEAN: f64 = 0.5;
const MAX_P99: f64 = 2.0;

/// decode reads a PNG as RGB8, dropping alpha when there is one.
fn decode(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut reader = png::Decoder::new(std::io::BufReader::new(file))
        .read_info()
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("image too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(format!("{}: not 8-bit", path.display()));
    }
    let px = &buf[..info.buffer_size()];
    let rgb = match info.color_type {
        png::ColorType::Rgb => px.to_vec(),
        png::ColorType::Rgba => px
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect(),
        other => return Err(format!("{}: unsupported {other:?}", path.display())),
    };
    Ok((info.width, info.height, rgb))
}

/// Row is one compared pair: its size and mean, p99 and max CIEDE2000.
struct Row {
    size: (u32, u32),
    mean: f64,
    p99: f64,
    max: f64,
}

/// compare measures one Mac/iPad pair: CIEDE2000 per pixel after decoding both to RGB8.
///
/// Differing dimensions are an error, not a score. The p99 is the value at rank ceil((n - 1) * 0.99)
/// of the ascending distances: at most one rank above nearest-rank, so it errs toward failing.
fn compare(a: &Path, b: &Path) -> Result<Row, String> {
    let ((wa, ha, pa), (wb, hb, pb)) = (decode(a)?, decode(b)?);
    if (wa, ha) != (wb, hb) {
        return Err(format!("size {wa}x{ha} vs {wb}x{hb}"));
    }
    let mut d: Vec<f64> = pa
        .as_chunks::<3>()
        .0
        .iter()
        .zip(pb.as_chunks::<3>().0)
        .map(|(x, y)| ciede2000(srgb8_to_lab(*x), srgb8_to_lab(*y)))
        .collect();
    let mean = d.iter().sum::<f64>() / d.len() as f64;
    d.sort_by(f64::total_cmp);
    let p99 = d[((d.len() - 1) as f64 * 0.99).ceil() as usize];
    Ok(Row {
        size: (wa, ha),
        mean,
        p99,
        max: d[d.len() - 1],
    })
}

/// main compares every PNG of the Mac directory with its namesake in the iPad directory and prints
/// the Markdown table. Exit 1 on any failing, missing or empty set, 2 on a wrong argument count.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mac, ipad] = args.as_slice() else {
        eprintln!("usage: compare_golden <mac_dir> <ipad_dir>");
        std::process::exit(2);
    };
    let mut names: Vec<String> = std::fs::read_dir(mac)
        .unwrap_or_else(|e| panic!("{mac}: {e}"))
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".png"))
        .collect();
    names.sort();

    println!("| file | preset | size | mean | p99 | max | verdict |");
    println!("|---|---|---|---|---|---|---|");
    let mut failed = 0;
    for name in &names {
        let (file, preset) = name
            .trim_end_matches(".png")
            .rsplit_once("__")
            .unwrap_or((name, "?"));
        match compare(&Path::new(mac).join(name), &Path::new(ipad).join(name)) {
            Ok(r) => {
                let pass = r.mean <= MAX_MEAN && r.p99 <= MAX_P99;
                if !pass {
                    failed += 1;
                }
                let (w, h) = r.size;
                let verdict = if pass { "PASS" } else { "FAIL" };
                println!(
                    "| {file} | {preset} | {w}x{h} | {:.4} | {:.4} | {:.4} | {verdict} |",
                    r.mean, r.p99, r.max
                );
            }
            Err(e) => {
                failed += 1;
                println!("| {file} | {preset} | - | - | - | - | FAIL: {e} |");
            }
        }
    }
    println!(
        "\n{} of {} pairs pass (mean <= {MAX_MEAN}, p99 <= {MAX_P99}).",
        names.len() - failed,
        names.len()
    );
    if names.is_empty() || failed > 0 {
        std::process::exit(1);
    }
}
