// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one pixel format everything converges on.
//!
//! Images, SVGs, PDF pages, and video frames all arrive from different
//! libraries in different layouts. Converting each of them to straight RGBA8 at
//! the point of production means the frontend has exactly one way to turn a
//! preview into something drawable, instead of four.

/// An 8-bit RGBA image, row-major, no padding.
#[derive(Clone, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes. Non-premultiplied.
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for Raster {
    /// The pixel buffer is megabytes; printing it turns a debug log into a core
    /// dump.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Raster")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

impl Raster {
    /// Wrap an existing RGBA buffer.
    ///
    /// Returns `None` when the buffer does not match the stated dimensions,
    /// which is the only way a downstream texture upload can go wrong in a way
    /// that is worth catching here rather than in the GPU driver.
    #[must_use]
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        (pixels.len() == expected).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Convert from cairo's ARGB32, which is what poppler renders into.
    ///
    /// Two conversions in one pass: cairo stores native-endian ARGB with
    /// premultiplied alpha, and everything downstream wants byte-order RGBA
    /// with straight alpha. Doing them separately would mean walking a
    /// multi-megabyte buffer twice.
    #[must_use]
    pub fn from_cairo_argb32(width: u32, height: u32, stride: usize, data: &[u8]) -> Option<Self> {
        let (w, h) = (width as usize, height as usize);
        if stride < w * 4 || data.len() < stride * h {
            return None;
        }

        let mut pixels = Vec::with_capacity(w * h * 4);
        for row in 0..h {
            let start = row * stride;
            for column in 0..w {
                let offset = start + column * 4;
                // Native-endian u32 rather than indexed bytes: on a big-endian
                // host the byte order is reversed, and cairo's format is
                // defined in terms of the word, not the bytes.
                let word = u32::from_ne_bytes([
                    data[offset],
                    data[offset + 1],
                    data[offset + 2],
                    data[offset + 3],
                ]);

                let a = ((word >> 24) & 0xff) as u8;
                let r = ((word >> 16) & 0xff) as u8;
                let g = ((word >> 8) & 0xff) as u8;
                let b = (word & 0xff) as u8;

                let (r, g, b) = unpremultiply(r, g, b, a);
                pixels.extend_from_slice(&[r, g, b, a]);
            }
        }

        Self::new(width, height, pixels)
    }

    /// Aspect ratio, or 1.0 for a degenerate image.
    #[must_use]
    pub fn aspect(&self) -> f32 {
        if self.height == 0 {
            return 1.0;
        }
        self.width as f32 / self.height as f32
    }

    /// Bytes occupied, for logging and for the decode budget.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.pixels.len()
    }
}

/// Recover straight-alpha colour from premultiplied.
///
/// Fully transparent pixels carry no colour to recover, so they stay black —
/// dividing by zero alpha would produce whatever the multiplication rounded to.
#[inline]
fn unpremultiply(r: u8, g: u8, b: u8, a: u8) -> (u8, u8, u8) {
    match a {
        0 => (0, 0, 0),
        255 => (r, g, b),
        alpha => {
            let scale = |channel: u8| (u32::from(channel) * 255 / u32::from(alpha)).min(255) as u8;
            (scale(r), scale(g), scale(b))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensions_must_match_the_buffer() {
        assert!(Raster::new(2, 2, vec![0; 16]).is_some());
        assert!(Raster::new(2, 2, vec![0; 15]).is_none());
    }

    #[test]
    fn opaque_pixels_survive_unpremultiplication() {
        assert_eq!(unpremultiply(10, 20, 30, 255), (10, 20, 30));
    }

    #[test]
    fn half_transparent_pixels_are_restored() {
        // 50% alpha halves each channel on the way in.
        let (r, g, b) = unpremultiply(50, 100, 150, 128);
        assert!((r as i32 - 99).abs() <= 1);
        assert!((g as i32 - 199).abs() <= 1);
        assert!((b as i32 - 255).abs() <= 1);
    }

    #[test]
    fn cairo_conversion_respects_stride() {
        // 1x1 image in a buffer padded to a 8-byte stride, opaque red.
        let mut data = vec![0u8; 8];
        data[..4].copy_from_slice(&0xffff_0000u32.to_ne_bytes());

        let raster = Raster::from_cairo_argb32(1, 1, 8, &data).expect("converts");
        assert_eq!(raster.pixels, vec![255, 0, 0, 255]);
    }

    #[test]
    fn a_short_cairo_buffer_is_rejected() {
        assert!(Raster::from_cairo_argb32(4, 4, 16, &[0; 8]).is_none());
    }
}
