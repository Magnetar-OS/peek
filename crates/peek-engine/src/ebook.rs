// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! EPUB previews: the cover, the bibliography, and readable text.
//!
//! An EPUB is a zip with a table of contents: `META-INF/container.xml` names
//! the package document, the package document names the metadata, the
//! manifest, and the reading order. Everything here follows those pointers
//! rather than guessing at file names, because the file names are the one
//! part of the format producers actually vary.
//!
//! Only the head of the book is read. A previewer answers "which book is
//! this, and is it the one I think it is" — the cover, the title, and the
//! opening pages answer that; chapter forty-one does not.

use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::raster::Raster;
use crate::text::{self, Line};

/// Lines of chapter text kept.
///
/// A few screenfuls. The cap is what keeps a thousand-page novel's preview
/// from reading the whole spine to fill a panel nobody scrolls to the end of.
pub const MAX_LINES: usize = 2_000;

/// Spine documents opened before stopping, whatever the line count.
///
/// Front matter is often dozens of near-empty XHTML files; this bounds the
/// walk when the line cap alone would keep it going.
const MAX_CHAPTERS: usize = 20;

/// Bytes of a single XHTML chapter read.
const MAX_CHAPTER_BYTES: u64 = 8 * 1024 * 1024;

/// Longest edge the cover is decoded to. The panel shows it at column width.
const COVER_EDGE: u32 = 1600;

/// What the previewer shows of an EPUB.
#[derive(Debug, Clone, Default)]
pub struct Ebook {
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub cover: Option<Raster>,
    /// The opening text, one paragraph per line, plain.
    pub lines: Vec<Line>,
    /// Set when the book holds more text than was read — which is nearly
    /// always, and worth saying anyway.
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the book is damaged: {0}")]
    Damaged(String),
}

/// Read an EPUB's cover, metadata, and opening text.
///
/// # Errors
///
/// Fails when the file cannot be read or its packaging is not intact enough
/// to locate the package document. A missing cover or empty metadata is not a
/// failure — the fields are simply absent.
pub fn load(path: &Path) -> Result<Ebook, Error> {
    let io = |source: std::io::Error| Error::Io {
        path: path.display().to_string(),
        source,
    };
    let damaged = |error: &dyn std::fmt::Display| Error::Damaged(error.to_string());

    let file = std::fs::File::open(path).map_err(io)?;
    let mut archive =
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| damaged(&e))?;

    let container = member_string(&mut archive, "META-INF/container.xml", MAX_CHAPTER_BYTES)
        .ok_or_else(|| Error::Damaged("no container.xml".to_owned()))?;
    let opf_path = rootfile(&container)
        .ok_or_else(|| Error::Damaged("container.xml names no package document".to_owned()))?;
    let opf = member_string(&mut archive, &opf_path, MAX_CHAPTER_BYTES)
        .ok_or_else(|| Error::Damaged(format!("missing package document {opf_path}")))?;

    let package = parse_package(&opf).map_err(|e| damaged(&e))?;

    // Hrefs in the package are relative to the package document itself.
    let base = opf_path
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);

    let cover = package.cover_href.as_deref().and_then(|href| {
        let bytes = member_bytes(&mut archive, &join(base, href), MAX_CHAPTER_BYTES)?;
        let decoded = image::load_from_memory(&bytes).ok()?;
        let reduced = crate::picture::reduce(decoded, COVER_EDGE);
        Raster::new(reduced.width(), reduced.height(), reduced.into_raw())
    });

    let mut lines: Vec<Line> = Vec::new();
    let mut chapters_read = 0;
    for href in &package.spine {
        if lines.len() >= MAX_LINES || chapters_read >= MAX_CHAPTERS {
            break;
        }
        let Some(xhtml) = member_string(&mut archive, &join(base, href), MAX_CHAPTER_BYTES) else {
            continue;
        };
        chapters_read += 1;

        for paragraph in chapter_text(&xhtml) {
            if lines.len() >= MAX_LINES {
                break;
            }
            lines.push(text::plain_line(&paragraph));
        }
    }

    let truncated = chapters_read < package.spine.len() || lines.len() >= MAX_LINES;

    Ok(Ebook {
        title: package.title,
        authors: package.authors,
        cover,
        lines,
        truncated,
    })
}

/// What the package document declares.
#[derive(Default)]
struct Package {
    title: Option<String>,
    authors: Vec<String>,
    /// Manifest href of the cover image, when one is declared.
    cover_href: Option<String>,
    /// Reading order, as hrefs of the XHTML documents.
    spine: Vec<String>,
}

/// The path of the package document, from `container.xml`.
fn rootfile(container: &str) -> Option<String> {
    let mut reader = Reader::from_str(container);
    while let Ok(event) = reader.read_event() {
        match event {
            Event::Eof => break,
            Event::Start(element) | Event::Empty(element)
                if element.local_name().as_ref() == "rootfile" =>
            {
                return attribute(&element, "full-path");
            }
            _ => {}
        }
    }
    None
}

/// Parse the package document: metadata, manifest, spine.
fn parse_package(opf: &str) -> Result<Package, quick_xml::Error> {
    struct Item {
        id: String,
        href: String,
        media_type: String,
        cover_image: bool,
    }

    let mut reader = Reader::from_str(opf);
    let mut package = Package::default();
    let mut items: Vec<Item> = Vec::new();
    let mut spine_ids: Vec<String> = Vec::new();
    // EPUB 2 declares the cover as `<meta name="cover" content="item-id"/>`.
    let mut cover_meta_id: Option<String> = None;
    // The dc: element currently being read, when it is one we keep.
    let mut reading: Option<&'static str> = None;

    loop {
        match reader.read_event()? {
            Event::Eof => break,

            Event::Start(element) => match element.local_name().as_ref() {
                "title" if package.title.is_none() => reading = Some("title"),
                "creator" => reading = Some("creator"),
                _ => {}
            },

            Event::End(_) => reading = None,

            Event::Text(value) => {
                let value = value.xml_content(XmlVersion::Implicit1_0);
                let value = value.trim();
                if value.is_empty() {
                    continue;
                }
                match reading {
                    Some("title") => package.title = Some(value.to_owned()),
                    Some("creator") => package.authors.push(value.to_owned()),
                    _ => {}
                }
            }

            Event::Empty(element) => match element.local_name().as_ref() {
                "item" => {
                    let properties = attribute(&element, "properties").unwrap_or_default();
                    if let (Some(id), Some(href)) =
                        (attribute(&element, "id"), attribute(&element, "href"))
                    {
                        items.push(Item {
                            id,
                            href,
                            media_type: attribute(&element, "media-type").unwrap_or_default(),
                            cover_image: properties.split_whitespace().any(|p| p == "cover-image"),
                        });
                    }
                }
                "itemref" => {
                    if let Some(idref) = attribute(&element, "idref") {
                        spine_ids.push(idref);
                    }
                }
                "meta" if attribute(&element, "name").as_deref() == Some("cover") => {
                    cover_meta_id = attribute(&element, "content");
                }
                _ => {}
            },

            _ => {}
        }
    }

    // EPUB 3's declaration wins; EPUB 2's meta is the fallback.
    package.cover_href = items
        .iter()
        .find(|item| item.cover_image)
        .or_else(|| {
            let id = cover_meta_id.as_deref()?;
            items.iter().find(|item| item.id == id)
        })
        .map(|item| item.href.clone());

    package.spine = spine_ids
        .iter()
        .filter_map(|id| {
            let item = items.iter().find(|item| &item.id == id)?;
            // Only documents: a spine can reference images and SVG directly.
            item.media_type.contains("xhtml").then(|| item.href.clone())
        })
        .collect();

    Ok(package)
}

/// The readable text of one XHTML chapter, one block element per paragraph.
fn chapter_text(xhtml: &str) -> Vec<String> {
    // Elements that end a paragraph of running text.
    const BLOCKS: &[&str] = &[
        "p",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "li",
        "div",
        "blockquote",
        "td",
    ];

    let mut reader = Reader::from_str(xhtml);
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    let mut in_body = false;

    let flush = |current: &mut String, paragraphs: &mut Vec<String>| {
        let text = current.trim();
        if !text.is_empty() {
            paragraphs.push(text.to_owned());
        }
        current.clear();
    };

    loop {
        match reader.read_event() {
            Ok(Event::Eof) | Err(_) => break,

            Ok(Event::Start(element)) => {
                if element.local_name().as_ref() == "body" {
                    in_body = true;
                }
            }

            Ok(Event::End(element)) => {
                let name = element.local_name();
                if name.as_ref() == "body" {
                    in_body = false;
                }
                if BLOCKS.contains(&name.as_ref()) {
                    flush(&mut current, &mut paragraphs);
                }
            }

            Ok(Event::Empty(element)) => {
                if element.local_name().as_ref() == "br" {
                    flush(&mut current, &mut paragraphs);
                }
            }

            Ok(Event::Text(value)) if in_body => {
                let content = value.xml_content(XmlVersion::Implicit1_0);
                let content =
                    quick_xml::escape::unescape(&content).unwrap_or_else(|_| content.clone());
                // Collapse the XML's own wrapping and indentation: a
                // paragraph broken across source lines is one paragraph,
                // and its newlines are the producer's formatting rather
                // than the author's.
                let mut words = content.split_whitespace().peekable();
                if words.peek().is_some() && !current.is_empty() {
                    current.push(' ');
                }
                let mut first = true;
                for word in words {
                    if !first {
                        current.push(' ');
                    }
                    current.push_str(word);
                    first = false;
                }
            }

            _ => {}
        }
    }

    flush(&mut current, &mut paragraphs);
    paragraphs
}

/// One attribute of an element, by local name.
fn attribute(element: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.local_name().as_ref() == name)
        .map(|attribute| attribute.value.into_owned())
}

/// Read a zip member as UTF-8 text, bounded.
fn member_string<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Option<String> {
    let mut text = String::new();
    archive
        .by_name(name)
        .ok()?
        .take(limit)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// Read a zip member's bytes, bounded.
fn member_bytes<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    archive
        .by_name(name)
        .ok()?
        .take(limit)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// Join a package-relative href onto the package document's directory.
fn join(base: &str, href: &str) -> String {
    if base.is_empty() {
        href.to_owned()
    } else {
        format!("{base}/{href}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A minimal but honest EPUB: container, package, two chapters, a cover.
    fn fixture(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();

        let mut add = |name: &str, content: &[u8]| {
            writer.start_file(name, options).expect("start");
            writer.write_all(content).expect("write");
        };

        add("mimetype", b"application/epub+zip");
        add(
            "META-INF/container.xml",
            br#"<?xml version="1.0"?>
            <container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
              <rootfiles>
                <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
              </rootfiles>
            </container>"#,
        );
        add(
            "OEBPS/content.opf",
            br#"<?xml version="1.0"?>
            <package xmlns="http://www.idpf.org/2007/opf" version="3.0">
              <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
                <dc:title>The Test Book</dc:title>
                <dc:creator>A. Writer</dc:creator>
              </metadata>
              <manifest>
                <item id="cover" href="cover.png" media-type="image/png" properties="cover-image"/>
                <item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
                <item id="c2" href="chapter2.xhtml" media-type="application/xhtml+xml"/>
              </manifest>
              <spine>
                <itemref idref="c1"/>
                <itemref idref="c2"/>
              </spine>
            </package>"#,
        );
        add(
            "OEBPS/chapter1.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml">
              <head><title>Ignored</title></head>
              <body><h1>Chapter One</h1><p>It began, as
              these things do, with a file.</p></body>
            </html>"#,
        );
        add(
            "OEBPS/chapter2.xhtml",
            br#"<html xmlns="http://www.w3.org/1999/xhtml">
              <body><p>And it continued.</p></body></html>"#,
        );

        let mut cover = Vec::new();
        let pixels = image::RgbaImage::from_pixel(20, 30, image::Rgba([5, 5, 200, 255]));
        image::DynamicImage::ImageRgba8(pixels)
            .write_to(
                &mut std::io::Cursor::new(&mut cover),
                image::ImageFormat::Png,
            )
            .expect("encode");
        add("OEBPS/cover.png", &cover);

        writer.finish().expect("finish");
        path
    }

    #[test]
    fn a_book_yields_cover_metadata_and_opening_text() {
        let path = fixture("peek-test-book.epub");
        let book = load(&path).expect("loads");

        assert_eq!(book.title.as_deref(), Some("The Test Book"));
        assert_eq!(book.authors, vec!["A. Writer".to_owned()]);

        let cover = book.cover.expect("a cover");
        assert_eq!((cover.width, cover.height), (20, 30));

        let text: Vec<String> = book.lines.iter().map(Line::plain).collect();
        assert_eq!(
            text,
            vec![
                "Chapter One".to_owned(),
                // The XML's own line wrapping collapses to a space.
                "It began, as these things do, with a file.".to_owned(),
                "And it continued.".to_owned(),
            ]
        );
        assert!(!book.truncated);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn head_titles_are_not_body_text() {
        let paragraphs = chapter_text(
            "<html><head><title>Not this</title></head><body><p>This.</p></body></html>",
        );
        assert_eq!(paragraphs, vec!["This.".to_owned()]);
    }

    #[test]
    fn a_zip_that_is_not_an_epub_is_damaged() {
        let path = std::env::temp_dir().join("peek-test-book-bad.epub");
        {
            let file = std::fs::File::create(&path).expect("create");
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            writer.start_file("whatever.txt", options).expect("start");
            writer.write_all(b"not a book").expect("write");
            writer.finish().expect("finish");
        }

        assert!(matches!(load(&path), Err(Error::Damaged(_))));
        let _ = std::fs::remove_file(path);
    }
}
