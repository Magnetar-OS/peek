// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Camera raw previews, from the JPEG the camera already embedded.
//!
//! Decoding raw sensor data is deliberately not attempted. A raw decode needs
//! per-camera colour matrices, demosaicing, and white balance — a photographic
//! pipeline, not a previewer — and would still show something different from
//! what the photographer saw on the back of the camera. What the camera saw is
//! *in the file*: every raw format embeds a full-size JPEG rendition, written
//! by the camera's own processor, and extracting it is what Quick Look shows
//! too.
//!
//! Nearly every raw format is a TIFF container — CR2, NEF, ARW, DNG, PEF, and
//! with slightly bent magic numbers ORF and RW2 — so one bounded IFD walk
//! covers them all. The walk collects every offset/length pair that could
//! locate an embedded JPEG, verifies the JPEG signature at each, and decodes
//! the largest. Fuji's RAF is the one non-TIFF holdout, and it stores the
//! preview's offset at a fixed position in its header.

use std::path::Path;

use crate::picture::{self, Picture};
use crate::raster::Raster;

/// Refuse to scan files beyond this size.
///
/// Raw files from current cameras are tens of megabytes; anything past this is
/// not a photograph, and reading it into memory to find out would be the exact
/// stall the previewer exists to avoid.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// Bounds on the IFD walk, against damaged or malicious files.
///
/// Real raw files have a handful of IFDs with tens of entries; the bounds are
/// an order of magnitude above that, so hitting one means the structure is
/// lying, not that the file is unusually rich.
const MAX_IFDS: usize = 64;
const MAX_ENTRIES: usize = 512;
const MAX_DEPTH: u8 = 4;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the file is {size} bytes, which is too large to scan")]
    TooLarge { size: u64 },
    #[error("no embedded preview was found")]
    NoPreview,
    #[error("could not decode the embedded preview: {0}")]
    Decode(#[from] image::ImageError),
}

/// Extract and decode the embedded preview of a camera raw file.
///
/// # Errors
///
/// Fails when the file cannot be read, carries no recognisable embedded JPEG,
/// or the JPEG itself cannot be decoded.
pub fn preview(path: &Path) -> Result<Picture, Error> {
    let io = |source: std::io::Error| Error::Io {
        path: path.display().to_string(),
        source,
    };

    let size = std::fs::metadata(path).map_err(io)?.len();
    if size > MAX_FILE_BYTES {
        return Err(Error::TooLarge { size });
    }

    // Read whole rather than seeked: the candidates can sit anywhere in the
    // file, and a raw file is small enough that one sequential read beats a
    // scatter of small ones.
    let data = std::fs::read(path).map_err(io)?;
    let jpeg = find_preview(&data).ok_or(Error::NoPreview)?;

    let mut decoded = image::load_from_memory(jpeg)?;

    // The embedded JPEG is usually stored unrotated, with the orientation in
    // the raw container's own IFD — which is exactly what the EXIF reader
    // parses, since a raw file *is* a TIFF with pictures attached.
    if let Some(orientation) = orientation(path) {
        decoded.apply_orientation(orientation);
    }

    let (source_width, source_height) = (decoded.width(), decoded.height());
    let reduced = picture::reduce(decoded, picture::MAX_EDGE);
    let raster = Raster::new(reduced.width(), reduced.height(), reduced.into_raw())
        .ok_or(Error::NoPreview)?;

    Ok(Picture {
        raster,
        source_width,
        source_height,
        exif: picture::exif_summary(path),
        scalable: false,
        animation: None,
    })
}

/// Locate the largest embedded JPEG in a raw container.
///
/// Returns the JPEG's bytes, borrowed from the file. `None` when the container
/// is not one this module understands or holds no verifiable JPEG.
fn find_preview(data: &[u8]) -> Option<&[u8]> {
    let mut candidates: Vec<(usize, usize)> = Vec::new();

    if let Some(candidate) = raf_preview(data) {
        candidates.push(candidate);
    } else {
        let tiff = Tiff::open(data)?;
        let mut ifds_seen = 0;
        tiff.walk(tiff.first_ifd, 0, &mut ifds_seen, &mut candidates);
    }

    // Verified before chosen: an offset that does not start with a JPEG
    // signature is raw sensor data wearing the wrong tag.
    candidates
        .into_iter()
        .filter(|&(offset, length)| {
            length >= 4
                && offset
                    .checked_add(length)
                    .is_some_and(|end| end <= data.len())
                && data[offset..].starts_with(&[0xff, 0xd8])
        })
        .max_by_key(|&(_, length)| length)
        .map(|(offset, length)| &data[offset..offset + length])
}

/// Fuji RAF: the preview's location is at a fixed header offset.
///
/// Bytes 84..88 carry the JPEG's offset and 88..92 its length, both big-endian,
/// after the `FUJIFILMCCD-RAW` magic.
fn raf_preview(data: &[u8]) -> Option<(usize, usize)> {
    if !data.starts_with(b"FUJIFILMCCD-RAW") || data.len() < 92 {
        return None;
    }
    let offset = u32::from_be_bytes(data[84..88].try_into().ok()?) as usize;
    let length = u32::from_be_bytes(data[88..92].try_into().ok()?) as usize;
    Some((offset, length))
}

/// A TIFF container, endianness resolved.
struct Tiff<'a> {
    data: &'a [u8],
    big_endian: bool,
    first_ifd: usize,
}

/// TIFF tags that can locate an embedded JPEG, and the structural ones.
const JPEG_OFFSET: u16 = 0x0201; // JPEGInterchangeFormat
const JPEG_LENGTH: u16 = 0x0202; // JPEGInterchangeFormatLength
const STRIP_OFFSETS: u16 = 0x0111;
const STRIP_BYTE_COUNTS: u16 = 0x0117;
const SUB_IFDS: u16 = 0x014a;
const JPG_FROM_RAW: u16 = 0x002e; // Panasonic RW2: the JPEG bytes themselves

impl<'a> Tiff<'a> {
    /// Parse the header, accepting the magic numbers raw formats actually use.
    ///
    /// 42 is TIFF proper (CR2, NEF, ARW, DNG, PEF share it); Olympus writes
    /// `RO`/`RS` and Panasonic writes 85, each into an otherwise ordinary
    /// header.
    fn open(data: &'a [u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        let big_endian = match &data[0..2] {
            b"II" => false,
            b"MM" => true,
            _ => return None,
        };

        let tiff = Self {
            data,
            big_endian,
            first_ifd: 0,
        };
        let magic = tiff.u16(2)?;
        if !matches!(magic, 42 | 85 | 0x4f52 | 0x5352) {
            return None;
        }

        let first_ifd = tiff.u32(4)? as usize;
        Some(Self { first_ifd, ..tiff })
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let bytes: [u8; 2] = self.data.get(at..at + 2)?.try_into().ok()?;
        Some(if self.big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let bytes: [u8; 4] = self.data.get(at..at + 4)?.try_into().ok()?;
        Some(if self.big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        })
    }

    /// Walk an IFD chain, collecting candidate JPEG locations.
    ///
    /// Follows the next-IFD pointer at the end of each IFD and recurses into
    /// `SubIFDs`, everything bounded — a damaged file can point IFDs at each
    /// other in a loop, and the walk has to end anyway.
    fn walk(
        &self,
        mut at: usize,
        depth: u8,
        ifds_seen: &mut usize,
        found: &mut Vec<(usize, usize)>,
    ) {
        while at != 0 && depth <= MAX_DEPTH && *ifds_seen < MAX_IFDS {
            *ifds_seen += 1;

            let Some(count) = self.u16(at) else { return };
            let count = (count as usize).min(MAX_ENTRIES);

            let mut jpeg_offset = None;
            let mut jpeg_length = None;
            let mut strip_offset = None;
            let mut strip_length = None;

            for index in 0..count {
                let entry = at + 2 + index * 12;
                let Some(tag) = self.u16(entry) else { return };
                let Some(kind) = self.u16(entry + 2) else {
                    return;
                };
                let Some(value_count) = self.u32(entry + 4) else {
                    return;
                };
                let Some(value) = self.u32(entry + 8) else {
                    return;
                };

                match tag {
                    JPEG_OFFSET => jpeg_offset = Some(value as usize),
                    JPEG_LENGTH => jpeg_length = Some(value as usize),
                    // Single-strip only: multi-strip data is the sensor dump,
                    // not a JPEG.
                    STRIP_OFFSETS if value_count == 1 => strip_offset = Some(value as usize),
                    STRIP_BYTE_COUNTS if value_count == 1 => strip_length = Some(value as usize),
                    // The value is an offset and the count is the byte length —
                    // the entry *is* the JPEG, in Panasonic's reading of TIFF.
                    JPG_FROM_RAW if kind == 7 => {
                        found.push((value as usize, value_count as usize));
                    }
                    SUB_IFDS => {
                        // One offset fits inline; more live behind the value.
                        let offsets = (value_count as usize).min(8);
                        for sub in 0..offsets {
                            let sub_at = if value_count == 1 {
                                Some(value as usize)
                            } else {
                                self.u32(value as usize + sub * 4).map(|v| v as usize)
                            };
                            if let Some(sub_at) = sub_at {
                                self.walk(sub_at, depth + 1, ifds_seen, found);
                            }
                        }
                    }
                    _ => {}
                }
            }

            if let (Some(offset), Some(length)) = (jpeg_offset, jpeg_length) {
                found.push((offset, length));
            }
            if let (Some(offset), Some(length)) = (strip_offset, strip_length) {
                found.push((offset, length));
            }

            match self.u32(at + 2 + count * 12) {
                Some(next) => at = next as usize,
                None => return,
            }
        }
    }
}

/// Read the container's orientation tag, mapped to the image crate's terms.
fn orientation(path: &Path) -> Option<image::metadata::Orientation> {
    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
    let field = exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?;
    let value = field.value.get_uint(0)?;
    image::metadata::Orientation::from_exif(value.try_into().ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a little-endian TIFF whose IFD0 points at an embedded JPEG.
    fn tiff_with_preview(jpeg: &[u8]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(b"II");
        data.extend_from_slice(&42u16.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes()); // first IFD at byte 8

        // IFD: two entries, then the JPEG appended after the table.
        let jpeg_at = 8 + 2 + 2 * 12 + 4;
        data.extend_from_slice(&2u16.to_le_bytes());

        let mut entry = |tag: u16, value: u32| {
            data.extend_from_slice(&tag.to_le_bytes());
            data.extend_from_slice(&4u16.to_le_bytes()); // LONG
            data.extend_from_slice(&1u32.to_le_bytes());
            data.extend_from_slice(&value.to_le_bytes());
        };
        entry(JPEG_OFFSET, jpeg_at as u32);
        entry(JPEG_LENGTH, jpeg.len() as u32);

        data.extend_from_slice(&0u32.to_le_bytes()); // no next IFD
        data.extend_from_slice(jpeg);
        data
    }

    fn tiny_jpeg() -> Vec<u8> {
        let mut jpeg = Vec::new();
        let image = image::RgbaImage::from_pixel(6, 4, image::Rgba([200, 10, 10, 255]));
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut jpeg),
                image::ImageFormat::Jpeg,
            )
            .expect("encode");
        jpeg
    }

    #[test]
    fn an_embedded_jpeg_is_found_and_verified() {
        let jpeg = tiny_jpeg();
        let data = tiff_with_preview(&jpeg);
        assert_eq!(find_preview(&data), Some(jpeg.as_slice()));
    }

    #[test]
    fn an_offset_pointing_at_non_jpeg_bytes_is_rejected() {
        // Same structure, but the payload is not a JPEG — the signature check
        // is what stops sensor data being handed to the decoder.
        let data = tiff_with_preview(b"not actually a jpeg");
        assert_eq!(find_preview(&data), None);
    }

    #[test]
    fn a_looping_ifd_chain_terminates() {
        let mut data = Vec::new();
        data.extend_from_slice(b"II");
        data.extend_from_slice(&42u16.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        // Zero entries, next-IFD pointer back to itself.
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());

        assert_eq!(find_preview(&data), None, "the walk must not spin forever");
    }

    #[test]
    fn a_raw_file_previews_end_to_end() {
        let path = std::env::temp_dir().join("peek-test-raw.nef");
        std::fs::write(&path, tiff_with_preview(&tiny_jpeg())).expect("write");

        let picture = preview(&path).expect("previews");
        assert_eq!(
            (picture.source_width, picture.source_height),
            (6, 4),
            "the embedded preview's own size is reported"
        );
        assert!(!picture.scalable);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_file_without_a_preview_says_so() {
        let path = std::env::temp_dir().join("peek-test-raw-empty.dng");
        // A valid TIFF header with an empty IFD: parseable, but nothing in it.
        let mut data = Vec::new();
        data.extend_from_slice(b"II");
        data.extend_from_slice(&42u16.to_le_bytes());
        data.extend_from_slice(&8u32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(&path, data).expect("write");

        assert!(matches!(preview(&path), Err(Error::NoPreview)));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn raf_headers_locate_the_preview_directly() {
        let jpeg = tiny_jpeg();
        let mut data = vec![0u8; 92];
        data[..15].copy_from_slice(b"FUJIFILMCCD-RAW");
        data[84..88].copy_from_slice(&(92u32).to_be_bytes());
        data[88..92].copy_from_slice(&(jpeg.len() as u32).to_be_bytes());
        data.extend_from_slice(&jpeg);

        assert_eq!(find_preview(&data), Some(jpeg.as_slice()));
    }
}
