// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! One representative image for a file.
//!
//! Two callers want exactly this and must not disagree: the `peek-thumbnailer`
//! binary, which answers the file manager's thumbnail requests, and the
//! overlay's index sheet, which shows a grid of the current selection. If they
//! each decided for themselves what a `.mp4` looks like, the icon in the file
//! manager and the cell in the grid would eventually drift apart — which is
//! the exact failure the shared engine exists to prevent.
//!
//! This is deliberately *not* a cache. Caching thumbnails is the calling
//! thumbnail factory's job — it owns `~/.cache/thumbnails`, the hashed names,
//! and the `Thumb::URI` metadata — and a second writer racing it would corrupt
//! what it wrote.

use crate::preview::{Options, Preview};
use crate::raster::Raster;
use crate::{Entry, picture};

/// Render a file down to one image, or decide it has none.
///
/// `size` is the longest edge in pixels. Formats that render at a requested
/// size — vectors, document pages — are rendered *at* it rather than produced
/// large and shrunk, which is the same reason the previewer passes a target
/// rather than a constant.
///
/// `None` for anything whose best representation is not a picture: text,
/// archives, directories, and files no previewer handles. The file manager's
/// own type icon says more about those than a rendering would, and declining
/// is how the thumbnailer contract says so.
#[must_use]
pub fn render(entry: &Entry, size: u32) -> Option<Raster> {
    let options = Options {
        // A thumbnail has no theme. The flag only selects a syntax palette,
        // and text does not thumbnail here.
        dark: false,
        vector_target: size,
        page_target: size,
        page: 0,
    };

    let raster = match crate::preview::load(entry, options) {
        Preview::Picture(picture) => picture.raster,
        #[cfg(feature = "pdf")]
        Preview::Pdf(pdf) => pdf.raster,
        // A comic's first page and a book's cover are what those files look
        // like — the same answer the overlay gives.
        Preview::Comic(comic) => comic.raster,
        Preview::Ebook(book) => book.cover?,
        // A video's poster frame, or an audio file's embedded cover art.
        #[cfg(feature = "media")]
        Preview::Media(media) => media.poster?,
        _ => return None,
    };

    // Bounded again here rather than trusted: only some of the previewers
    // above honour the target, and a 4K PDF page in a 128 px grid cell is
    // several megabytes uploaded to draw a postage stamp.
    if raster.width <= size && raster.height <= size {
        return Some(raster);
    }

    let image = image::RgbaImage::from_raw(raster.width, raster.height, raster.pixels)?;
    let reduced = picture::reduce(image::DynamicImage::ImageRgba8(image), size);
    Raster::new(reduced.width(), reduced.height(), reduced.into_raw())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str, width: u32, height: u32) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        image::RgbaImage::from_pixel(width, height, image::Rgba([20, 160, 90, 255]))
            .save(&path)
            .expect("encode");
        path
    }

    #[test]
    fn an_image_is_reduced_to_the_requested_edge() {
        let path = fixture("peek-test-thumb.png", 600, 300);
        let entry = Entry::load(&path).expect("stats");

        let raster = render(&entry, 128).expect("a thumbnail");
        assert_eq!((raster.width, raster.height), (128, 64), "aspect is kept");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_small_image_is_not_enlarged() {
        let path = fixture("peek-test-thumb-small.png", 32, 32);
        let entry = Entry::load(&path).expect("stats");

        let raster = render(&entry, 256).expect("a thumbnail");
        assert_eq!((raster.width, raster.height), (32, 32));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn text_declines_rather_than_rendering_something_meaningless() {
        let path = std::env::temp_dir().join("peek-test-thumb.txt");
        std::fs::write(&path, b"hello\n").expect("write");
        let entry = Entry::load(&path).expect("stats");

        assert!(render(&entry, 128).is_none());

        let _ = std::fs::remove_file(path);
    }
}
