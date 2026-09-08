// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Report what `peek` makes of a file, without opening a window.
//!
//! Media is the one subsystem that cannot be checked by a unit test: whether a
//! file decodes depends on which GStreamer plugins are installed, and whether it
//! *plays* depends on an audio device and a working pipeline. Neither exists in
//! a build container, so the tests only assert that a non-media file does not
//! panic. This is how the rest is checked.
//!
//! ```sh
//! cargo run -p peek-engine --example probe -- clip.mp4 photo.jpg
//! ```
//!
//! Also useful for triage: if the overlay shows a metadata card where a preview
//! was expected, running this on the same file says whether the fault is in the
//! decoder or in the frontend.

use std::path::PathBuf;
#[cfg(feature = "media")]
use std::time::Duration;

#[cfg(feature = "media")]
use peek_engine::Player;
use peek_engine::{Entry, Options, Preview};

/// How long playback is allowed to run before the position is sampled.
///
/// Long enough for a pipeline to leave `Paused` and produce a frame; short
/// enough that probing a directory is not a chore.
#[cfg(feature = "media")]
const SETTLE: Duration = Duration::from_millis(900);

fn main() {
    let paths: Vec<PathBuf> = std::env::args().skip(1).map(PathBuf::from).collect();

    if paths.is_empty() {
        eprintln!("usage: probe <file>…");
        std::process::exit(2);
    }

    for path in paths {
        let Some(entry) = Entry::load(&path) else {
            println!("{}: cannot be read", path.display());
            continue;
        };

        println!("{}", entry.name);
        println!("  type      {} ({:?})", entry.mime, entry.kind);
        println!("  size      {}", peek_engine::meta::size(entry.size));

        match peek_engine::preview::load(&entry, Options::default()) {
            Preview::Picture(picture) => {
                println!(
                    "  image     {}×{} source, {}×{} decoded",
                    picture.source_width,
                    picture.source_height,
                    picture.raster.width,
                    picture.raster.height
                );
                for (label, value) in &picture.exif {
                    println!("  exif      {label}: {value}");
                }
            }
            #[cfg(feature = "pdf")]
            Preview::Pdf(pdf) => println!(
                "  pdf       page {} of {}, rendered {}×{}",
                pdf.page + 1,
                pdf.pages,
                pdf.raster.width,
                pdf.raster.height
            ),
            Preview::Text(document) => println!(
                "  text      {} lines, {:?}{}",
                document.lines.len(),
                document.language,
                if document.truncated {
                    ", truncated"
                } else {
                    ""
                }
            ),
            Preview::Archive(archive) => println!(
                "  archive   {}, {} members, {} uncompressed",
                archive.format,
                archive.members.len(),
                peek_engine::meta::size(archive.total_size)
            ),
            Preview::Directory(directory) => println!(
                "  folder    {} files, {} folders",
                directory.files, directory.directories
            ),
            Preview::Comic(comic) => println!(
                "  comic     page {} of {}, rendered {}×{}",
                comic.page + 1,
                comic.pages,
                comic.raster.width,
                comic.raster.height
            ),
            Preview::Ebook(book) => {
                println!(
                    "  ebook     {:?} by {}",
                    book.title,
                    if book.authors.is_empty() {
                        "unknown".to_owned()
                    } else {
                        book.authors.join(", ")
                    }
                );
                println!(
                    "  cover     {}",
                    book.cover.as_ref().map_or_else(
                        || "none".to_owned(),
                        |cover| format!("{}×{}", cover.width, cover.height)
                    )
                );
                println!("  text      {} lines", book.lines.len());
            }
            Preview::Font(font) => println!(
                "  font      {} {:?}, {} glyphs, variable={} mono={} faces={}",
                font.family, font.style, font.glyphs, font.variable, font.monospaced, font.faces
            ),
            #[cfg(feature = "media")]
            Preview::Media(media) => report_media(&path, &media),
            Preview::Card(_) => println!("  card      no previewer applies"),
            Preview::Failed { reason, detail, .. } => {
                println!("  failed    {reason:?}: {detail}");
            }
            Preview::Pending => println!("  pending   (unreachable from a blocking load)"),
        }

        println!();
    }
}

/// Describe a media file and then actually play it.
///
/// Playback is exercised rather than just probed because the two fail
/// independently: a file can describe itself perfectly through the discoverer
/// and still produce no frames, which is what a missing decoder plugin looks
/// like from the outside.
#[cfg(feature = "media")]
fn report_media(path: &std::path::Path, media: &peek_engine::Media) {
    println!(
        "  media     decodable={} video={} audio={}",
        media.decodable, media.has_video, media.has_audio
    );
    if let Some(duration) = media.duration {
        println!("  duration  {}", peek_engine::meta::duration(duration));
    }
    if media.has_video {
        println!("  size      {}×{}", media.width, media.height);
    }
    for stream in &media.streams {
        println!("  stream    {stream}");
    }
    for (label, value) in &media.tags {
        println!("  tag       {label}: {value}");
    }
    println!(
        "  poster    {}",
        media.poster.as_ref().map_or_else(
            || "none".to_owned(),
            |p| format!("{}×{}", p.width, p.height)
        )
    );

    if !media.decodable {
        return;
    }

    let Ok(player) = Player::open(path, media.has_video) else {
        println!("  playback  could not build a pipeline");
        return;
    };

    player.play();
    std::thread::sleep(SETTLE);

    println!(
        "  playback  playing={} position={:?}",
        player.is_playing(),
        player.position()
    );
    println!(
        "  frame     {}",
        player.frame().map_or_else(
            || "none".to_owned(),
            |f| format!("{}×{}", f.width, f.height)
        )
    );

    // A seek is the one control that silently does nothing when the pipeline
    // cannot seek the container, so it is worth reporting separately.
    player.seek(Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(500));
    println!("  seek      position={:?}", player.position());
}
