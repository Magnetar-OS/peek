// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! PDF previews, rendered through poppler.
//!
//! ## Why poppler
//!
//! It is the only PDF renderer already installed on a Linux desktop. PDFium is
//! permissively licensed but ships as a binary that has to be fetched
//! separately, and MuPDF is AGPL, which is a heavier obligation than poppler's
//! GPL. Depending on the library the desktop already has means the previewer
//! renders a PDF exactly the way the document viewer next to it does.
//!
//! ## How the document is held
//!
//! `poppler::Document` is a GObject and is not `Send`, so it cannot sit in the
//! application's state and be rendered from whichever worker thread is free.
//! [`render`] therefore opens the document fresh — which is what the first
//! preview of a file does, on a thread that then moves on — and [`Session`]
//! exists for everything after that: a dedicated thread that owns the document
//! and answers page requests over a channel. One header parse per document,
//! and a page turn costs only the rendering.

use std::path::Path;

use crate::raster::Raster;

/// Largest edge a rendered page may be rasterised to.
///
/// Pages are rendered at display resolution rather than at the document's
/// nominal 72 dpi, because a PDF at 1:1 is unreadably small on a modern
/// display and scaling it up afterwards would show the rasteriser's pixels
/// rather than the document's type.
///
/// A ceiling rather than a target: the caller passes the size the page will
/// actually occupy, and this only stops a 4K panel at 2× from asking poppler
/// for a raster larger than any texture it is worth uploading.
pub const MAX_EDGE: f64 = 4400.0;

/// Smallest edge a rendered page may be rasterised to.
///
/// A caller that has not learned its geometry yet must not be handed a page
/// rendered at forty pixels; below this the fixed default is better than the
/// number that was asked for.
const MIN_EDGE: f64 = 900.0;

/// A rendered page, plus what the document says about itself.
#[derive(Debug, Clone)]
pub struct Pdf {
    pub raster: Raster,
    /// Zero-based index of the rendered page.
    pub page: usize,
    pub pages: usize,
    pub title: Option<String>,
    pub author: Option<String>,
    /// Page size in PostScript points, which is what a print dialog shows.
    pub point_width: f64,
    pub point_height: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not open the PDF: {0}")]
    Open(glib::Error),
    #[error("the PDF is encrypted and needs a password")]
    Encrypted,
    #[error("the PDF has no pages")]
    Empty,
    #[error("page {0} does not exist")]
    NoSuchPage(usize),
    #[error("could not rasterise the page: {0}")]
    Render(#[from] cairo::Error),
    #[error("could not read back the rendered page: {0}")]
    Readback(#[from] cairo::BorrowError),
    #[error("the rendered page was empty")]
    Blank,
    #[error("the document session is gone")]
    Closed,
}

/// Render one page of a PDF.
///
/// `target` is the longest edge, in physical pixels, the page will be drawn at.
/// It is clamped to [`MIN_EDGE`]..=[`MAX_EDGE`], so a caller that does not know
/// its geometry yet still gets a legible page.
///
/// # Errors
///
/// Fails when the file is not a PDF poppler can open, is password-protected,
/// has no pages, or the requested page is out of range.
pub fn render(path: &Path, page: usize, target: u32) -> Result<Pdf, Error> {
    let document = open_document(path)?;
    render_page(&document, page, target)
}

/// Open a document, distinguishing the one failure the user can fix.
fn open_document(path: &Path) -> Result<poppler::Document, Error> {
    let uri = glib::filename_to_uri(path, None).map_err(Error::Open)?;

    // `None` password: an encrypted document is reported rather than guessed
    // at. Prompting for a password inside a previewer that a keypress opened
    // would be a surprising place to be asked for one.
    poppler::Document::from_file(&uri, None).map_err(|error| {
        // poppler reports a password requirement as an ordinary error; the
        // distinction matters because it is the one failure the user can fix.
        if error.matches(poppler::Error::Encrypted) {
            Error::Encrypted
        } else {
            Error::Open(error)
        }
    })
}

/// Rasterise one page of an already-open document.
fn render_page(document: &poppler::Document, page: usize, target: u32) -> Result<Pdf, Error> {
    let pages = document.n_pages().max(0) as usize;
    if pages == 0 {
        return Err(Error::Empty);
    }
    if page >= pages {
        return Err(Error::NoSuchPage(page));
    }

    let rendered = document.page(page as i32).ok_or(Error::NoSuchPage(page))?;

    let (point_width, point_height) = rendered.size();
    if point_width <= 0.0 || point_height <= 0.0 {
        return Err(Error::Blank);
    }

    let edge = f64::from(target).clamp(MIN_EDGE, MAX_EDGE);
    let scale = edge / point_width.max(point_height);
    let width = (point_width * scale).round().max(1.0) as i32;
    let height = (point_height * scale).round().max(1.0) as i32;

    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)?;
    {
        let context = cairo::Context::new(&surface)?;
        // Paint white first. A PDF page has no background of its own, and
        // rendering onto transparency shows the panel through the paper —
        // which looks like a rendering bug rather than like a design choice.
        context.set_source_rgb(1.0, 1.0, 1.0);
        context.paint()?;
        context.scale(scale, scale);
        rendered.render(&context);
        // The context has to be dropped before the surface data can be
        // borrowed; cairo tracks that as an exclusive borrow at runtime.
    }
    surface.flush();

    let stride = surface.stride().max(0) as usize;
    let data = surface.data()?;

    let raster = Raster::from_cairo_argb32(width as u32, height as u32, stride, &data)
        .ok_or(Error::Blank)?;

    Ok(Pdf {
        raster,
        page,
        pages,
        title: non_empty(document.title().map(Into::into)),
        author: non_empty(document.author().map(Into::into)),
        point_width,
        point_height,
    })
}

/// Drop empty metadata strings.
///
/// Poppler returns `Some("")` for fields a producer wrote but left blank, which
/// would otherwise render as a labelled row with nothing in it.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|text| !text.trim().is_empty())
}

/// A held-open document that renders pages on request.
///
/// The document lives on a dedicated thread — the only way to keep a
/// non-`Send` GObject across calls — and this handle is a channel to it, so
/// the handle itself is `Send`, `Sync`, and cheap to clone. Dropping the last
/// handle closes the channel, which ends the thread and the document with it.
#[derive(Clone)]
pub struct Session {
    sender: std::sync::mpsc::Sender<Request>,
}

struct Request {
    page: usize,
    target: u32,
    reply: std::sync::mpsc::SyncSender<Result<Pdf, Error>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").finish_non_exhaustive()
    }
}

impl Session {
    /// Open a document and keep it open.
    ///
    /// Never fails here: the open happens on the session's own thread, and a
    /// document that cannot be opened simply ends that thread, after which
    /// [`Session::render`] reports [`Error::Closed`]. The caller's fallback for
    /// that — a fresh [`render`] — is also what produces the precise reason.
    #[must_use]
    pub fn open(path: &Path) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel::<Request>();
        let path = path.to_path_buf();

        let spawned = std::thread::Builder::new()
            .name("peek-pdf".to_owned())
            .spawn(move || {
                let Ok(document) = open_document(&path) else {
                    // Dropping the receiver is the signal; the reason is
                    // re-derived by the caller's fallback path.
                    return;
                };
                while let Ok(request) = receiver.recv() {
                    let _ =
                        request
                            .reply
                            .send(render_page(&document, request.page, request.target));
                }
            });

        if let Err(error) = spawned {
            tracing::warn!(%error, "could not start the document session thread");
        }

        Self { sender }
    }

    /// Render a page through the held document.
    ///
    /// Blocking — rendering is CPU work and this call is made from a thread
    /// that is allowed to wait on it.
    ///
    /// # Errors
    ///
    /// Fails as [`render`] does, plus [`Error::Closed`] when the document could
    /// not be opened or the session thread is gone.
    pub fn render(&self, page: usize, target: u32) -> Result<Pdf, Error> {
        let (reply, response) = std::sync::mpsc::sync_channel(1);
        self.sender
            .send(Request {
                page,
                target,
                reply,
            })
            .map_err(|_| Error::Closed)?;
        response.recv().map_err(|_| Error::Closed)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_pdf_is_an_error_rather_than_a_panic() {
        let path = std::env::temp_dir().join("peek-test-not.pdf");
        std::fs::write(&path, b"this is not a PDF").expect("write");

        assert!(render(&path, 0, 1650).is_err());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_file_is_an_error() {
        assert!(render(Path::new("/nonexistent/peek/doc.pdf"), 0, 1650).is_err());
    }

    #[test]
    fn blank_metadata_is_dropped() {
        assert_eq!(non_empty(Some("   ".to_owned())), None);
        assert_eq!(
            non_empty(Some("Title".to_owned())),
            Some("Title".to_owned())
        );
    }

    #[test]
    fn a_session_on_an_unopenable_file_reports_closed() {
        let path = std::env::temp_dir().join("peek-test-session.pdf");
        std::fs::write(&path, b"not a pdf").expect("write");

        let session = Session::open(&path);
        assert!(matches!(session.render(0, 1650), Err(Error::Closed)));

        let _ = std::fs::remove_file(path);
    }
}
