// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Choosing and running a previewer.
//!
//! Everything here is synchronous and blocking on purpose. Decoding an image,
//! rasterising a PDF page, and walking a tar are all CPU work, and wrapping CPU
//! work in `async` buys nothing — it just moves the stall onto the executor.
//! The frontend runs [`load`] on a blocking thread and receives the finished
//! [`Preview`] as a message, which is the same shape `light` uses for its file
//! index.
//!
//! ## Failure is a preview, not an error
//!
//! [`load`] has no error type. A corrupt JPEG, a password-protected PDF, and an
//! archive with a damaged header all resolve to [`Preview::Failed`], which
//! carries the metadata card alongside the reason. The user pressed a key over a
//! file that exists; showing them its size, type, and modification date plus
//! "could not decode" is strictly more useful than showing them nothing, and it
//! means the overlay never has an empty state to design for.

use crate::archive::Archive;
use crate::comic::Comic;
use crate::ebook::Ebook;
use crate::entry::Entry;
use crate::font::FontSpecimen;
use crate::kind::Kind;
#[cfg(feature = "media")]
use crate::media::Media;
use crate::meta::{Card, Directory};
#[cfg(feature = "pdf")]
use crate::pdf::Pdf;
use crate::picture::Picture;
use crate::text::Document;

/// How a file should be drawn.
#[derive(Debug, Clone)]
pub enum Preview {
    /// Nothing has been decoded yet. Held while the blocking load runs, so the
    /// overlay can open immediately rather than after the file is ready.
    Pending,
    Picture(Box<Picture>),
    Text(Box<Document>),
    #[cfg(feature = "pdf")]
    Pdf(Box<Pdf>),
    /// A comic book archive, paged like a PDF.
    Comic(Box<Comic>),
    /// An EPUB: cover, metadata, and opening text.
    Ebook(Box<Ebook>),
    /// A font, described for the specimen the frontend draws.
    Font(Box<FontSpecimen>),
    /// Audio or video. Playback is not started here — see
    /// [`crate::media::Player`].
    #[cfg(feature = "media")]
    Media(Box<Media>),
    Archive(Box<Archive>),
    Directory(Box<Directory>),
    /// The fallback: everything a file can be described by.
    Card(Box<Card>),
    /// A previewer was chosen and did not work. The card is still shown; the
    /// reason says what went wrong in terms the frontend can word.
    Failed {
        reason: Reason,
        /// The underlying error, rendered. For logs and for the `probe`
        /// example — the overlay shows the worded [`Reason`] instead, because
        /// a `thiserror` message is English and the user may not be.
        detail: String,
        card: Box<Card>,
    },
}

/// Why a previewer declined, as a fact rather than a sentence.
///
/// The same rule as [`crate::meta::Field`]: the engine knows *which* failure
/// happened, and the frontend chooses the words — it is the only layer that
/// knows what language the user reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The file could not be read at all.
    Unreadable,
    /// The bytes could not be decoded as what they claimed to be.
    Undecodable,
    /// The file is encrypted and needs a password.
    Encrypted,
    /// Decoding was refused because it would take too long.
    TooLarge,
    /// The file decoded, but to nothing worth showing.
    Empty,
    /// Nothing on this system can decode the streams it holds.
    NoCodec,
    /// The container carries no embedded preview to extract.
    NoPreview,
}

impl Preview {
    /// Whether this preview fills the surface rather than sitting in a column.
    #[must_use]
    pub fn is_visual(&self) -> bool {
        if matches!(self, Self::Picture(_) | Self::Comic(_)) {
            return true;
        }
        #[cfg(feature = "pdf")]
        if matches!(self, Self::Pdf(_)) {
            return true;
        }
        #[cfg(feature = "media")]
        if matches!(self, Self::Media(media) if media.has_video) {
            return true;
        }
        false
    }

    /// The content's own pixel size, when enlarging past it would be wrong.
    ///
    /// `None` for anything that can be re-rendered at any size — vectors, PDF
    /// pages — and for video, where filling the panel is what a player does.
    /// A raster returns its real dimensions, so a small image opens small
    /// rather than as a blurry rectangle the size of the display.
    #[must_use]
    pub fn natural_size(&self) -> Option<(u32, u32)> {
        match self {
            Self::Picture(picture) if !picture.scalable => {
                Some((picture.source_width, picture.source_height))
            }
            _ => None,
        }
    }

    /// Aspect ratio of the content, when it has one.
    ///
    /// The overlay sizes itself to this so a portrait photo does not open in a
    /// landscape window with bars down both sides — which is the single most
    /// visible difference between a previewer and an image viewer.
    #[must_use]
    pub fn aspect(&self) -> Option<f32> {
        match self {
            Self::Picture(picture) => Some(picture.raster.aspect()),
            Self::Comic(comic) => Some(comic.raster.aspect()),
            #[cfg(feature = "pdf")]
            Self::Pdf(pdf) => Some(pdf.raster.aspect()),
            #[cfg(feature = "media")]
            Self::Media(media) if media.has_video && media.height > 0 => {
                Some(media.width as f32 / media.height as f32)
            }
            #[cfg(feature = "media")]
            Self::Media(media) => media.poster.as_ref().map(crate::raster::Raster::aspect),
            _ => None,
        }
    }
}

/// What the caller wants from a load.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Whether the desktop is in dark mode, which selects the syntax palette.
    pub dark: bool,
    /// Longest edge, in pixels, an SVG should be rendered at. Vectors are
    /// rendered to fit rather than scaled, so this has to come from the display.
    ///
    /// Physical pixels, not logical: the texture is uploaded and sampled at the
    /// display's real resolution, and a target in logical pixels renders a
    /// vector at half size on a 2× display.
    pub vector_target: u32,
    /// Longest edge, in pixels, a document page should be rasterised at.
    ///
    /// Same units and the same reason as [`Options::vector_target`]. Separate
    /// from it because a page is drawn into the panel rather than across the
    /// display, so the two are bounded by different things.
    pub page_target: u32,
    /// Which page of a multi-page document to render.
    pub page: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            dark: true,
            // Sane defaults for a 1080p display at 1×; the frontend overrides
            // both with the real surface size and scale factor.
            vector_target: 1920,
            page_target: 1650,
            page: 0,
        }
    }
}

/// Decode a file into something drawable.
///
/// Blocking. Run it on a thread that is allowed to block for hundreds of
/// milliseconds — decoding a 60 megapixel photo or rasterising a dense PDF page
/// is not fast, and pretending otherwise only hides the cost.
#[must_use]
pub fn load(entry: &Entry, options: Options) -> Preview {
    let card = || Box::new(crate::meta::card(entry));

    // Every arm has the same shape: try the specialised previewer, fall back to
    // the card with the reason attached. Written out rather than abstracted
    // because each previewer takes different arguments and the abstraction
    // would be larger than the repetition.
    match entry.kind {
        Kind::Directory => Preview::Directory(Box::new(crate::meta::directory(&entry.path))),

        Kind::Image => match crate::picture::raster(&entry.path) {
            Ok(picture) => Preview::Picture(Box::new(picture)),
            Err(error) => failed(reason_for_picture(&error), error, card()),
        },

        Kind::CameraRaw => match crate::raw::preview(&entry.path) {
            Ok(picture) => Preview::Picture(Box::new(picture)),
            Err(error) => failed(reason_for_raw(&error), error, card()),
        },

        Kind::Vector => match crate::picture::vector(&entry.path, options.vector_target) {
            Ok(picture) => Preview::Picture(Box::new(picture)),
            Err(error) => failed(reason_for_picture(&error), error, card()),
        },

        #[cfg(feature = "pdf")]
        Kind::Pdf => match crate::pdf::render(&entry.path, options.page, options.page_target) {
            Ok(pdf) => Preview::Pdf(Box::new(pdf)),
            Err(error) => failed(reason_for_pdf(&error), error, card()),
        },

        Kind::Text => match crate::text::load(&entry.path, options.dark) {
            Ok(document) => Preview::Text(Box::new(document)),
            // Reading is the only way a text preview fails.
            Err(error) => failed(Reason::Unreadable, error, card()),
        },

        Kind::Markdown => match crate::markdown::render(&entry.path, options.dark) {
            Ok(document) => Preview::Text(Box::new(document)),
            Err(error) => failed(Reason::Unreadable, error, card()),
        },

        Kind::Comic => match crate::comic::render(&entry.path, options.page, options.page_target) {
            Ok(comic) => Preview::Comic(Box::new(comic)),
            Err(error) => failed(reason_for_comic(&error), error, card()),
        },

        Kind::Ebook => match crate::ebook::load(&entry.path) {
            Ok(ebook) => Preview::Ebook(Box::new(ebook)),
            Err(error) => failed(reason_for_ebook(&error), error, card()),
        },

        Kind::Office => match crate::office::text(&entry.path, &entry.mime) {
            // The reading column already knows how to show a document; an
            // office file's text is a document like any other.
            Ok(document) => Preview::Text(Box::new(document)),
            Err(error) => failed(reason_for_office(&error), error, card()),
        },

        Kind::Font => match crate::font::specimen(&entry.path) {
            Ok(font) => Preview::Font(Box::new(font)),
            Err(error) => failed(reason_for_font(&error), error, card()),
        },

        Kind::Archive => match crate::archive::list(&entry.path, &entry.mime) {
            Ok(archive) => Preview::Archive(Box::new(archive)),
            Err(error) => failed(reason_for_archive(&error), error, card()),
        },

        #[cfg(feature = "media")]
        Kind::Video | Kind::Audio => match crate::media::probe(&entry.path) {
            Ok(mut media) => {
                // Cover art is only read for audio, and only once the file is
                // the one actually on screen — it costs an image decode.
                if entry.kind == Kind::Audio && media.poster.is_none() {
                    media.poster = crate::media::cover_art(&entry.path);
                }
                Preview::Media(Box::new(media))
            }
            Err(error) => failed(reason_for_media(&error), error, card()),
        },

        Kind::Plugin => plugin_preview(entry, options, card),

        // With the previewer that renders them not compiled in, these types are
        // still *detected* — the card names the type, the size, and the date,
        // which is an honest answer rather than a missing one. Gating detection
        // as well would make a `default-features = false` build claim a PDF was
        // an unknown blob.
        #[cfg(not(feature = "pdf"))]
        Kind::Pdf => Preview::Card(card()),
        #[cfg(not(feature = "media"))]
        Kind::Video | Kind::Audio => Preview::Card(card()),

        Kind::Other => Preview::Card(card()),
    }
}

/// Run whichever plugin claimed this file.
///
/// The plugin's result is mapped onto the previews that already exist — a
/// rendered image becomes a [`Preview::Picture`], extracted text becomes a
/// [`Preview::Text`] — so a plugin adds file *types* without adding anything
/// the frontend has to learn to draw.
fn plugin_preview(entry: &Entry, options: Options, card: impl Fn() -> Box<Card>) -> Preview {
    use crate::plugin::{Handler, Output};

    let Some(rule) = crate::plugin::registry().find(&entry.mime, &entry.path) else {
        // The registry answered differently than it did during detection,
        // which means a plugin file changed underneath a running daemon.
        return Preview::Card(card());
    };

    match rule.handler {
        Handler::Text => {
            match crate::text::load_with_syntax(&entry.path, options.dark, rule.syntax.as_deref()) {
                Ok(document) => Preview::Text(Box::new(document)),
                Err(error) => failed(Reason::Unreadable, error, card()),
            }
        }
        Handler::Image => match crate::picture::raster(&entry.path) {
            Ok(picture) => Preview::Picture(Box::new(picture)),
            Err(error) => failed(reason_for_picture(&error), error, card()),
        },
        Handler::Vector => match crate::picture::vector(&entry.path, options.vector_target) {
            Ok(picture) => Preview::Picture(Box::new(picture)),
            Err(error) => failed(reason_for_picture(&error), error, card()),
        },
        Handler::Archive => match crate::archive::list(&entry.path, &entry.mime) {
            Ok(archive) => Preview::Archive(Box::new(archive)),
            Err(error) => failed(reason_for_archive(&error), error, card()),
        },
        #[cfg(not(feature = "media"))]
        Handler::Media => Preview::Card(card()),
        #[cfg(feature = "media")]
        Handler::Media => match crate::media::probe(&entry.path) {
            Ok(media) => Preview::Media(Box::new(media)),
            Err(error) => failed(reason_for_media(&error), error, card()),
        },
        Handler::Command => {
            let product = match crate::plugin::run(rule, &entry.path, options.page_target) {
                Ok(product) => product,
                Err(error) => return failed(Reason::Undecodable, error, card()),
            };

            match rule.output {
                Output::Image => match crate::picture::raster(product.path()) {
                    Ok(picture) => Preview::Picture(Box::new(picture)),
                    Err(error) => failed(reason_for_picture(&error), error, card()),
                },
                // The output's path is a scratch name that says nothing about
                // its contents, so the rule's declared syntax is the only
                // evidence there is.
                Output::Text => match crate::text::load_with_syntax(
                    product.path(),
                    options.dark,
                    rule.syntax.as_deref(),
                ) {
                    Ok(document) => Preview::Text(Box::new(document)),
                    Err(error) => failed(Reason::Unreadable, error, card()),
                },
            }
        }
    }
}

fn failed(reason: Reason, error: impl std::fmt::Display, card: Box<Card>) -> Preview {
    let detail = error.to_string();
    tracing::debug!(%detail, ?reason, "falling back to the metadata card");
    Preview::Failed {
        reason,
        detail,
        card,
    }
}

// One mapping per previewer, next to the dispatch that uses them. Written as
// functions rather than `From` impls because the mapping is this module's
// opinion about presentation, not a property of the error types.

fn reason_for_picture(error: &crate::picture::Error) -> Reason {
    use crate::picture::Error;
    match error {
        Error::Io { .. } => Reason::Unreadable,
        Error::Decode(_) | Error::Svg(_) => Reason::Undecodable,
        Error::TooLarge { .. } => Reason::TooLarge,
        Error::Empty => Reason::Empty,
    }
}

fn reason_for_raw(error: &crate::raw::Error) -> Reason {
    use crate::raw::Error;
    match error {
        Error::Io { .. } => Reason::Unreadable,
        Error::TooLarge { .. } => Reason::TooLarge,
        Error::NoPreview => Reason::NoPreview,
        Error::Decode(_) => Reason::Undecodable,
    }
}

#[cfg(feature = "pdf")]
fn reason_for_pdf(error: &crate::pdf::Error) -> Reason {
    use crate::pdf::Error;
    match error {
        Error::Encrypted => Reason::Encrypted,
        Error::Empty | Error::NoSuchPage(_) => Reason::Empty,
        Error::Open(_) | Error::Render(_) | Error::Readback(_) | Error::Blank | Error::Closed => {
            Reason::Undecodable
        }
    }
}

fn reason_for_archive(error: &crate::archive::Error) -> Reason {
    use crate::archive::Error;
    match error {
        Error::Io { .. } => Reason::Unreadable,
        Error::Unsupported | Error::Damaged(_) => Reason::Undecodable,
    }
}

fn reason_for_comic(error: &crate::comic::Error) -> Reason {
    use crate::comic::Error;
    match error {
        Error::Io { .. } => Reason::Unreadable,
        Error::Damaged(_) | Error::Decode(_) | Error::Blank => Reason::Undecodable,
        Error::NoPages | Error::NoSuchPage(_) => Reason::Empty,
        Error::PageTooLarge { .. } => Reason::TooLarge,
    }
}

fn reason_for_ebook(error: &crate::ebook::Error) -> Reason {
    use crate::ebook::Error;
    match error {
        Error::Io { .. } => Reason::Unreadable,
        Error::Damaged(_) => Reason::Undecodable,
    }
}

fn reason_for_office(error: &crate::office::Error) -> Reason {
    use crate::office::Error;
    match error {
        Error::Io { .. } => Reason::Unreadable,
        Error::Damaged(_) | Error::Unsupported => Reason::Undecodable,
    }
}

fn reason_for_font(error: &crate::font::Error) -> Reason {
    use crate::font::Error;
    match error {
        Error::Io { .. } => Reason::Unreadable,
        Error::TooLarge { .. } => Reason::TooLarge,
        Error::Parse(_) => Reason::Undecodable,
    }
}

#[cfg(feature = "media")]
fn reason_for_media(error: &crate::media::Error) -> Reason {
    use crate::media::Error;
    match error {
        Error::Uri(_) => Reason::Unreadable,
        Error::Undecodable => Reason::NoCodec,
        Error::Init(_) | Error::Pipeline(_) => Reason::Undecodable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(name);
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(bytes).expect("write");
        path
    }

    #[test]
    fn an_undecodable_image_still_previews_as_a_card() {
        let path = write("peek-test-broken.png", b"\x89PNG\r\n\x1a\n truncated");
        let entry = Entry::load(&path).expect("stats");

        // Detection sees PNG magic, so the image previewer is chosen and fails —
        // which is exactly the path this test exists to pin down.
        let preview = load(&entry, Options::default());
        assert!(
            matches!(preview, Preview::Failed { .. }),
            "expected a card with a reason, got {preview:?}"
        );

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_unknown_binary_previews_as_a_card() {
        let path = write("peek-test-unknown.blob", &[0x00, 0x01, 0x02, 0x03]);
        let entry = Entry::load(&path).expect("stats");

        assert!(matches!(load(&entry, Options::default()), Preview::Card(_)));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn text_previews_as_text() {
        let path = write("peek-test-preview.txt", b"hello\nworld\n");
        let entry = Entry::load(&path).expect("stats");

        match load(&entry, Options::default()) {
            Preview::Text(document) => assert_eq!(document.lines.len(), 2),
            other => panic!("expected text, got {other:?}"),
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_directory_previews_as_a_summary() {
        let entry = Entry::load(&std::env::temp_dir()).expect("stats");
        assert!(matches!(
            load(&entry, Options::default()),
            Preview::Directory(_)
        ));
    }

    #[test]
    fn text_has_no_aspect_ratio_to_size_the_panel_by() {
        assert!(Preview::Text(Box::default()).aspect().is_none());
    }
}
