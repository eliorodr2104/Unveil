// Modified for Unveil (2026-10-17): memory ownership and reserve policy; see Engine/CHANGES.md.
//! Display output owns either CPU pixels or a GPU staging mapping until delivery completes.

use lightcraft_raster::Rgba8;

/// RGBA8 output with a scoped pixel view. A mapped crop keeps the original row stride and starts
/// at its first pixel. All `height * stride` bytes are readable; only `width * 4` bytes per row
/// are image pixels. Any tail needed after a crop's final row is reserved by the producer.
pub struct DisplayImage {
    pub width: usize,
    pub height: usize,
    storage: Storage,
}

enum Storage {
    Owned(Rgba8),
    #[cfg(not(target_arch = "wasm32"))]
    Mapped {
        readback: crate::ctx::MappedReadback,
        stride: usize,
        offset: usize,
    },
}

impl From<Rgba8> for DisplayImage {
    fn from(image: Rgba8) -> Self {
        Self { width: image.width, height: image.height, storage: Storage::Owned(image) }
    }
}

impl DisplayImage {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn mapped(readback: crate::ctx::MappedReadback, width: usize, height: usize, stride: usize, offset: usize) -> Self {
        Self { width, height, storage: Storage::Mapped { readback, stride, offset } }
    }

    /// Whether delivery still uses mapped staging storage rather than an owned CPU image.
    pub fn is_mapped(&self) -> bool {
        match self.storage {
            Storage::Owned(_) => false,
            #[cfg(not(target_arch = "wasm32"))]
            Storage::Mapped { .. } => true,
        }
    }

    /// Borrow pixels only for this call. No GPU work or wait occurs here: the producer has already
    /// completed and validated the readback. `stride` is measured in bytes.
    pub fn with_pixels<R>(&self, consume: impl FnOnce(&[u8], usize) -> R) -> Result<R, String> {
        match &self.storage {
            Storage::Owned(image) => {
                if (self.width, self.height) != (image.width, image.height) {
                    return Err("display output dimensions changed".into());
                }
                Ok(consume(image.data.as_flattened(), image.width * 4))
            }
            #[cfg(not(target_arch = "wasm32"))]
            Storage::Mapped { readback, stride, offset } => readback
                .with_bytes(|bytes| {
                    if self.width.checked_mul(4)? > *stride {
                        return None;
                    }
                    let length = self.height.checked_mul(*stride)?;
                    let region = bytes.get(*offset..offset.checked_add(length)?)?;
                    Some(consume(region, *stride))
                })?
                .ok_or_else(|| "mapped output region is out of bounds".into()),
        }
    }

    /// Materialize only for consumers that need owned pixels, such as export and cache encoding.
    pub fn into_owned(self) -> Result<Rgba8, String> {
        if let Storage::Owned(image) = self.storage {
            return Ok(image);
        }
        self.with_pixels(|bytes, stride| {
            if stride == self.width * 4 {
                return Rgba8 { width: self.width, height: self.height, data: bytes.as_chunks::<4>().0.to_vec().into() };
            }
            let mut data = Vec::with_capacity(self.width * self.height);
            for y in 0..self.height {
                data.extend_from_slice(bytes[y * stride..y * stride + self.width * 4].as_chunks::<4>().0);
            }
            Rgba8 { width: self.width, height: self.height, data: data.into() }
        })
    }
}
