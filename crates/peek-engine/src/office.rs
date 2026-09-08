// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Office documents, reduced to the question a previewer answers.
//!
//! A previewer's question is "what is in this file", and for a word-processing
//! document the answer is its text. Full-fidelity layout would mean embedding
//! an office suite, which is a non-goal — so DOCX and ODT, both of which are
//! zipped XML, have their text streamed out of the XML and shown through the
//! same reading column source files use. The card says what the file is; the
//! text says what it contains.
//!
//! Extraction matches on local element names, not prefixes: `w:t` is
//! WordprocessingML's text run by convention, but the prefix is the producer's
//! choice and the namespace is what means it.

use std::io::Read;
use std::path::Path;

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::text::{self, Document};

/// Bytes of document XML read before stopping.
///
/// The XML of a prose document is small; tens of megabytes of it is a
/// generated report nobody is reading in a preview panel.
const MAX_XML_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the document is damaged: {0}")]
    Damaged(String),
    #[error("this document format is not one the text extractor reads")]
    Unsupported,
}

/// Which of the two zipped-XML dialects a type is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    /// WordprocessingML: display text lives only inside `w:t`. Everything
    /// else in a paragraph — field codes, deleted runs, properties — is
    /// machinery.
    Word,
    /// OpenDocument: all text inside a paragraph or heading is display text,
    /// spans and links included.
    OpenDocument,
}

/// The zip member that holds a format's document XML, and how to read it.
fn body_member(mime: &str) -> Option<(&'static str, Dialect)> {
    match mime {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            Some(("word/document.xml", Dialect::Word))
        }
        "application/vnd.oasis.opendocument.text" => Some(("content.xml", Dialect::OpenDocument)),
        _ => None,
    }
}

/// Extract a document's text into a plain [`Document`].
///
/// # Errors
///
/// Fails when the file cannot be read, is not the zip its type claims, or its
/// document XML is missing or malformed.
pub fn text(path: &Path, mime: &str) -> Result<Document, Error> {
    let (member, dialect) = body_member(mime).ok_or(Error::Unsupported)?;

    let io = |source: std::io::Error| Error::Io {
        path: path.display().to_string(),
        source,
    };

    let file = std::fs::File::open(path).map_err(io)?;
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|error| Error::Damaged(error.to_string()))?;

    let entry = archive
        .by_name(member)
        .map_err(|error| Error::Damaged(error.to_string()))?;

    let mut xml = String::new();
    entry
        .take(MAX_XML_BYTES)
        .read_to_string(&mut xml)
        .map_err(io)?;

    let lines = extract(&xml, dialect).map_err(|error| Error::Damaged(error.to_string()))?;
    let truncated = lines.len() >= text::MAX_LINES;
    let bytes_read = xml.len();

    Ok(Document {
        lines: lines.iter().map(|line| text::plain_line(line)).collect(),
        language: None,
        truncated,
        bytes_read,
        lossy: false,
        // Extracted document text is prose: it is read, not inspected.
        prose: true,
    })
}

/// Walk the document XML, one paragraph per line.
///
/// The same walk serves both formats because the elements that matter carry
/// the same local names or near enough: text lives in text events, `p` (and
/// ODF's `h`) ends a paragraph, `tab` is a tab, `br` and ODF's `line-break`
/// break a line in place, and ODF's `s` is a run of spaces.
fn extract(xml: &str, dialect: Dialect) -> Result<Vec<String>, quick_xml::Error> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    // Depth of `w:t` / ODF paragraph nesting we are inside — text outside it
    // is instructions and metadata, not prose.
    let mut capturing: u32 = 0;
    let push_line = |lines: &mut Vec<String>, current: &mut String| {
        if lines.len() < text::MAX_LINES {
            lines.push(std::mem::take(current));
        }
    };

    loop {
        if lines.len() >= text::MAX_LINES {
            break;
        }

        match reader.read_event()? {
            Event::Eof => break,

            Event::Start(element) => match element.local_name().as_ref() {
                // The element that turns text into *display* text differs by
                // dialect — see [`Dialect`]. Getting this wrong is how field
                // codes end up in the preview.
                "t" if dialect == Dialect::Word => capturing += 1,
                "p" | "h" if dialect == Dialect::OpenDocument => capturing += 1,
                _ => {}
            },

            Event::End(element) => match element.local_name().as_ref() {
                "t" if dialect == Dialect::Word => capturing = capturing.saturating_sub(1),
                "p" | "h" => {
                    if dialect == Dialect::OpenDocument {
                        capturing = capturing.saturating_sub(1);
                    }
                    push_line(&mut lines, &mut current);
                }
                _ => {}
            },

            Event::Empty(element) => match element.local_name().as_ref() {
                "tab" => current.push('\t'),
                "br" | "line-break" => push_line(&mut lines, &mut current),
                // ODF compresses runs of spaces into `<text:s text:c="N"/>`.
                "s" => {
                    let count = element
                        .attributes()
                        .flatten()
                        .find(|attribute| attribute.key.local_name().as_ref() == "c")
                        .and_then(|attribute| attribute.value.parse::<usize>().ok())
                        .unwrap_or(1);
                    current.push_str(&" ".repeat(count.min(256)));
                }
                _ => {}
            },

            Event::Text(value) if capturing > 0 => {
                let content = value.xml_content(XmlVersion::Implicit1_0);
                // Entities are the producer's escaping, not the author's
                // text: `&amp;` in the file is an ampersand on the page.
                match quick_xml::escape::unescape(&content) {
                    Ok(unescaped) => current.push_str(&unescaped),
                    Err(_) => current.push_str(&content),
                }
            }

            _ => {}
        }
    }

    if !current.is_empty() {
        push_line(&mut lines, &mut current);
    }

    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(member: &str, xml: &str, name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        let file = std::fs::File::create(&path).expect("create");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        writer.start_file(member, options).expect("start");
        writer.write_all(xml.as_bytes()).expect("write");
        writer.finish().expect("finish");
        path
    }

    const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
    const ODT: &str = "application/vnd.oasis.opendocument.text";

    #[test]
    fn docx_paragraphs_become_lines() {
        let xml = r#"<?xml version="1.0"?>
            <w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
              <w:body>
                <w:p><w:r><w:t>First paragraph.</w:t></w:r></w:p>
                <w:p><w:r><w:t>Second, with a</w:t></w:r><w:r><w:tab/><w:t>tab.</w:t></w:r></w:p>
              </w:body>
            </w:document>"#;
        let path = zip_with("word/document.xml", xml, "peek-test-office.docx");

        let document = text(&path, DOCX).expect("extracts");
        assert_eq!(document.lines.len(), 2);
        assert_eq!(document.lines[0].plain(), "First paragraph.");
        assert_eq!(document.lines[1].plain(), "Second, with a\ttab.");
        assert!(!document.truncated);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn odt_headings_and_paragraphs_become_lines() {
        let xml = r#"<?xml version="1.0"?>
            <office:document-content
                xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
                xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
              <office:body><office:text>
                <text:h>Title</text:h>
                <text:p>Body with<text:s text:c="3"/>spaces.</text:p>
              </office:text></office:body>
            </office:document-content>"#;
        let path = zip_with("content.xml", xml, "peek-test-office.odt");

        let document = text(&path, ODT).expect("extracts");
        assert_eq!(document.lines.len(), 2);
        assert_eq!(document.lines[0].plain(), "Title");
        assert_eq!(document.lines[1].plain(), "Body with   spaces.");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn instruction_text_outside_runs_is_not_prose() {
        // Text sitting outside `w:t` — field codes, properties — must not leak
        // into the extraction.
        let xml = r#"<w:document xmlns:w="ns"><w:body>
            <w:p><w:instrText>PAGEREF _Toc1</w:instrText><w:r><w:t>Real text.</w:t></w:r></w:p>
        </w:body></w:document>"#;
        let path = zip_with("word/document.xml", xml, "peek-test-office-instr.docx");

        let document = text(&path, DOCX).expect("extracts");
        assert_eq!(document.lines[0].plain(), "Real text.");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_zip_without_the_document_xml_is_damaged() {
        let path = zip_with(
            "mimetype",
            "application/whatever",
            "peek-test-office-empty.docx",
        );
        assert!(matches!(text(&path, DOCX), Err(Error::Damaged(_))));
        let _ = std::fs::remove_file(path);
    }
}
