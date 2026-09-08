// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Type detection and preview rendering for the `peek` previewer.
//!
//! Everything a previewer frontend needs that is not drawing: deciding what a
//! file is, turning it into pixels or lines or a listing, and describing it when
//! none of those apply. There are no UI dependencies here, so a second frontend
//! — a GTK4 one, for the desktops where `wlr-layer-shell` is not available —
//! links this crate rather than reimplementing it.
//!
//! ```no_run
//! use peek_engine::{Entry, Neighbourhood, Options, Preview, preview};
//!
//! // The file manager names one file; the directory supplies the rest.
//! let mut around = Neighbourhood::around(std::path::Path::new("/tmp/photo.jpg"));
//! let entry = Entry::load(around.current().expect("a current file")).expect("stats");
//!
//! // Blocking: run this off the frame path.
//! match preview::load(&entry, Options::default()) {
//!     Preview::Picture(picture) => {
//!         let _ = (picture.raster.width, picture.raster.height);
//!     }
//!     Preview::Failed { detail, .. } => eprintln!("{detail}"),
//!     _ => {}
//! }
//!
//! // Right arrow moves to the next file in the same directory.
//! around.step(1);
//! ```

pub mod archive;
pub mod comic;
pub mod ebook;
pub mod entry;
pub mod font;
pub mod kind;
pub mod markdown;
#[cfg(feature = "media")]
pub mod media;
pub mod meta;
pub mod office;
#[cfg(feature = "pdf")]
pub mod pdf;
pub mod picture;
pub mod plugin;
pub mod preview;
pub mod raster;
pub mod raw;
pub mod text;
pub mod thumbnail;

pub use archive::Archive;
pub use comic::Comic;
pub use ebook::Ebook;
pub use entry::{Entry, Neighbourhood};
pub use font::FontSpecimen;
pub use kind::Kind;
#[cfg(feature = "media")]
pub use media::{Media, Player};
pub use meta::{Card, Directory};
#[cfg(feature = "pdf")]
pub use pdf::Pdf;
pub use picture::Picture;
pub use plugin::Plugin;
pub use preview::{Options, Preview};
pub use raster::Raster;
pub use text::Document;
