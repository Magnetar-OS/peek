// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Font previews: the facts for a specimen card.
//!
//! The engine parses; it does not rasterise. A specimen — the pangram at
//! several sizes — is text drawn *in* the font, and drawing text is the one
//! thing the frontend's rendering stack already does better than anything
//! this crate could bolt on. So the specimen carries the file's bytes for the
//! frontend to register with its own text renderer, alongside the names and
//! counts the card states.

use std::path::Path;

/// Refuse to parse files beyond this size.
///
/// CJK fonts are tens of megabytes; past this the file is not a font that
/// should be read into memory to find out.
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// What the specimen shows about a font file.
#[derive(Debug, Clone)]
pub struct FontSpecimen {
    /// Family name, preferring the typographic family over the legacy one —
    /// "Source Serif 4" rather than "Source Serif 4 Display Semibold".
    pub family: String,
    /// Style name: "Bold Italic", "Display", whatever the designer wrote.
    pub style: Option<String>,
    pub version: Option<String>,
    pub glyphs: u16,
    /// Whether the file carries variation axes.
    pub variable: bool,
    pub monospaced: bool,
    /// Faces in the file. One for a plain font; more for a collection, of
    /// which the first is what the specimen describes.
    pub faces: u32,
    /// The file's bytes, for the frontend to register with its renderer.
    pub data: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the file is {size} bytes, which is too large to parse as a font")]
    TooLarge { size: u64 },
    #[error("could not parse the font: {0}")]
    Parse(ttf_parser::FaceParsingError),
}

/// Parse a font file far enough to describe it.
///
/// # Errors
///
/// Fails when the file cannot be read or is not a font `ttf-parser`
/// understands — which includes WOFF and WOFF2, whose compression it does not
/// unpack; detection keeps those away from this previewer.
pub fn specimen(path: &Path) -> Result<FontSpecimen, Error> {
    let io = |source: std::io::Error| Error::Io {
        path: path.display().to_string(),
        source,
    };

    let size = std::fs::metadata(path).map_err(io)?.len();
    if size > MAX_FILE_BYTES {
        return Err(Error::TooLarge { size });
    }

    let data = std::fs::read(path).map_err(io)?;
    let faces = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
    let face = ttf_parser::Face::parse(&data, 0).map_err(Error::Parse)?;

    let name = |id: u16| -> Option<String> {
        face.names()
            .into_iter()
            .filter(|name| name.name_id == id)
            .find_map(|name| name.to_string())
            .filter(|value| !value.trim().is_empty())
    };

    let family = name(ttf_parser::name_id::TYPOGRAPHIC_FAMILY)
        .or_else(|| name(ttf_parser::name_id::FAMILY))
        .unwrap_or_else(|| {
            // A font with no readable family still previews; the file name is
            // the frontend's fallback and the card carries the honest blank.
            String::new()
        });

    let style = name(ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY)
        .or_else(|| name(ttf_parser::name_id::SUBFAMILY));

    Ok(FontSpecimen {
        family,
        style,
        version: name(ttf_parser::name_id::VERSION),
        glyphs: face.number_of_glyphs(),
        variable: face.is_variable(),
        monospaced: face.is_monospaced(),
        faces,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A system font, when the test machine has one.
    ///
    /// Font parsing needs a real font, and shipping one as a fixture would
    /// test the fixture. Skipped silently where none exists (a bare build
    /// container) — the parse-failure path below runs everywhere.
    fn system_font() -> Option<std::path::PathBuf> {
        let roots = ["/usr/share/fonts", "/usr/local/share/fonts"];
        for root in roots {
            let mut stack = vec![std::path::PathBuf::from(root)];
            while let Some(dir) = stack.pop() {
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                        e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf")
                    }) {
                        return Some(path);
                    }
                }
            }
        }
        None
    }

    #[test]
    fn a_real_font_yields_a_family_and_glyphs() {
        let Some(path) = system_font() else {
            eprintln!("no system font found; skipping");
            return;
        };

        let specimen = specimen(&path).expect("parses");
        assert!(!specimen.family.is_empty(), "a family name was expected");
        assert!(specimen.glyphs > 0);
        assert!(specimen.faces >= 1);
        assert!(!specimen.data.is_empty());
    }

    #[test]
    fn a_non_font_is_an_error_not_a_panic() {
        let path = std::env::temp_dir().join("peek-test-not-a-font.ttf");
        std::fs::write(&path, b"definitely not sfnt data").expect("write");

        assert!(matches!(specimen(&path), Err(Error::Parse(_))));

        let _ = std::fs::remove_file(path);
    }
}
