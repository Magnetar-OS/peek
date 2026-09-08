// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Comic book archives, read as the paged documents they are.
//!
//! A CBZ is images in a zip, and the reading order is the natural sort of the
//! file names — the same order the archive's author counted the pages in and
//! the same comparison the file manager shows them with. Only the requested
//! page is decoded; the listing is the zip's central directory, so opening a
//! six-hundred-page archive costs one page's decode, not six hundred.
//!
//! Nothing is extracted, for the same reason the archive previewer extracts
//! nothing.

use std::io::Read;
use std::path::Path;

use crate::raster::Raster;

/// Largest edge a decoded page is reduced to, whatever the caller asks for.
const MAX_EDGE: u32 = crate::picture::MAX_EDGE;

/// Smallest edge a page is rendered at, for callers that have not learned
/// their geometry yet. Same reasoning as the PDF previewer's floor.
const MIN_EDGE: u32 = 900;

/// Refuse to decode a single page beyond this compressed size.
///
/// A page is a scan, and scans are megabytes — tens of them means the entry is
/// not a page, whatever its extension says.
const MAX_PAGE_BYTES: u64 = 128 * 1024 * 1024;

/// One rendered page, plus where it sits in the book.
#[derive(Debug, Clone)]
pub struct Comic {
    pub raster: Raster,
    /// Zero-based index of the rendered page.
    pub page: usize,
    pub pages: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the archive is damaged: {0}")]
    Damaged(String),
    #[error("the archive holds no pages")]
    NoPages,
    #[error("page {0} does not exist")]
    NoSuchPage(usize),
    #[error("page {page} is {size} bytes, which is too large to decode")]
    PageTooLarge { page: usize, size: u64 },
    #[error("could not decode the page: {0}")]
    Decode(#[from] image::ImageError),
    #[error("the decoded page was empty")]
    Blank,
}

/// Extensions that count as pages.
///
/// Anything else in the archive — `ComicInfo.xml`, thumbnails, folder
/// metadata — is not a page and must not shift the numbering.
const PAGE_EXTENSIONS: &[&str] = &[
    "avif", "bmp", "gif", "jpeg", "jpg", "png", "tif", "tiff", "webp",
];

/// Render one page of a comic archive.
///
/// `target` is the longest edge, in physical pixels, the page will be drawn
/// at, clamped the same way a PDF page's is.
///
/// # Errors
///
/// Fails when the file cannot be read or is not a zip, holds no images, or
/// the requested page cannot be decoded.
pub fn render(path: &Path, page: usize, target: u32) -> Result<Comic, Error> {
    let file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|error| Error::Damaged(error.to_string()))?;

    let mut pages = page_names(&mut archive);
    if pages.is_empty() {
        return Err(Error::NoPages);
    }
    pages.sort_by(|a, b| crate::entry::natural(a, b));

    let total = pages.len();
    let name = pages.into_iter().nth(page).ok_or(Error::NoSuchPage(page))?;

    let mut entry = archive
        .by_name(&name)
        .map_err(|error| Error::Damaged(error.to_string()))?;

    let size = entry.size();
    if size > MAX_PAGE_BYTES {
        return Err(Error::PageTooLarge { page, size });
    }

    let mut bytes = Vec::with_capacity(size as usize);
    entry.read_to_end(&mut bytes).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;

    let decoded = image::load_from_memory(&bytes)?;
    let reduced = crate::picture::reduce(decoded, target.clamp(MIN_EDGE, MAX_EDGE));
    let raster =
        Raster::new(reduced.width(), reduced.height(), reduced.into_raw()).ok_or(Error::Blank)?;

    Ok(Comic {
        raster,
        page,
        pages: total,
    })
}

/// The archive members that are pages, unordered.
fn page_names<R: std::io::Read + std::io::Seek>(archive: &mut zip::ZipArchive<R>) -> Vec<String> {
    (0..archive.len())
        .filter_map(|index| {
            let entry = archive.by_index_raw(index).ok()?;
            if entry.is_dir() {
                return None;
            }
            let name = entry.name();

            // Resource forks and hidden files are bookkeeping, not pages.
            if name.starts_with("__MACOSX/")
                || name
                    .rsplit('/')
                    .next()
                    .is_some_and(|leaf| leaf.starts_with('.'))
            {
                return None;
            }

            let extension = name.rsplit('.').next()?.to_lowercase();
            PAGE_EXTENSIONS
                .contains(&extension.as_str())
                .then(|| name.to_owned())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A zip of numbered pages plus the metadata files real comics carry.
    fn fixture(name: &str, shades: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        // Named so lexicographic order would be wrong: page 10 before page 2.
        for (index, shade) in shades.iter().enumerate() {
            let number = index + 1;
            // Two single-digit pages then double digits exercises the natural
            // sort: "page10" must come after "page2".
            let member = format!("page{number}.png");
            writer.start_file(member, options).expect("start");

            let mut bytes = Vec::new();
            let page = image::RgbaImage::from_pixel(8, 12, image::Rgba([*shade, 0, 0, 255]));
            image::DynamicImage::ImageRgba8(page)
                .write_to(
                    &mut std::io::Cursor::new(&mut bytes),
                    image::ImageFormat::Png,
                )
                .expect("encode");
            writer.write_all(&bytes).expect("write");
        }

        writer.start_file("ComicInfo.xml", options).expect("start");
        writer.write_all(b"<ComicInfo/>").expect("write");
        writer.finish().expect("finish");

        path
    }

    #[test]
    fn pages_are_counted_and_rendered_in_natural_order() {
        let path = fixture(
            "peek-test-comic.cbz",
            &[10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120],
        );

        let comic = render(&path, 0, 900).expect("renders");
        assert_eq!(comic.pages, 12, "metadata files are not pages");
        assert_eq!(comic.page, 0);
        assert_eq!(comic.raster.pixels[0], 10, "page 1 is the first page");

        // Natural order: page 10 is the tenth page, not the second.
        let tenth = render(&path, 9, 900).expect("renders");
        assert_eq!(tenth.raster.pixels[0], 100);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_out_of_range_page_is_an_error() {
        let path = fixture("peek-test-comic-range.cbz", &[1, 2]);
        assert!(matches!(render(&path, 5, 900), Err(Error::NoSuchPage(5))));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_zip_with_no_images_reports_no_pages() {
        let path = std::env::temp_dir().join("peek-test-comic-empty.cbz");
        {
            let file = std::fs::File::create(&path).expect("create");
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            writer.start_file("notes.txt", options).expect("start");
            writer.write_all(b"no pages here").expect("write");
            writer.finish().expect("finish");
        }

        assert!(matches!(render(&path, 0, 900), Err(Error::NoPages)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_non_zip_is_damaged_not_a_panic() {
        let path = std::env::temp_dir().join("peek-test-comic-bad.cbz");
        std::fs::write(&path, b"not a zip at all").expect("write");
        assert!(matches!(render(&path, 0, 900), Err(Error::Damaged(_))));
        let _ = std::fs::remove_file(path);
    }
}
