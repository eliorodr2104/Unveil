//! Exact histogram and completeness summary written by the reduced-output dispatch.

use crate::ctx::Buf;
use crate::render::Cx;
use lightcraft_raster::Histogram;

pub(crate) const TILE: usize = 4;
pub(crate) const WORDS: usize = 1026;

pub(crate) struct ReducedStats {
    pub buffer: Buf,
    pub step: usize,
    pixels: usize,
    samples: u32,
}

impl ReducedStats {
    pub fn new(cx: &mut Cx<'_>, width: usize, height: usize) -> Self {
        let step = ((width as f64 * height as f64 / 1_000_000.0).sqrt().ceil() as usize).max(1);
        Self { buffer: cx.zeroed(WORDS), step, pixels: width * height, samples: (width.div_ceil(step) * height.div_ceil(step)) as u32 }
    }

    pub fn decode(&self, bytes: &[u8]) -> Result<(usize, Histogram), String> {
        let words: &[u32] = bytemuck::try_cast_slice(bytes).map_err(|e| format!("reduced statistics alignment: {e}"))?;
        if words.len() != WORDS || words[1025] as usize != self.pixels || words[1024] as usize > self.pixels {
            return Err("the GPU returned incomplete reduced statistics".into());
        }
        if !words[..1024].chunks_exact(256).all(|bins| bins.iter().map(|v| *v as u64).sum::<u64>() == self.samples as u64) {
            return Err("the GPU returned inconsistent reduced histogram totals".into());
        }
        Ok((
            words[1024] as usize,
            Histogram {
                r: words[..256].to_vec(),
                g: words[256..512].to_vec(),
                b: words[512..768].to_vec(),
                luma: words[768..1024].to_vec(),
                total: self.samples,
            },
        ))
    }
}
