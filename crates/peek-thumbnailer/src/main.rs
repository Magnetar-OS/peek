// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! `peek-thumbnailer` — the file manager's icons, from the previewer's eyes.
//!
//! This is Quick Look's actual architecture on macOS: one framework renders
//! both the thumbnail in the Finder and the preview over it, so the two can
//! never disagree about what a file looks like. Here the shared half is
//! `peek-engine`, and this binary is the thin adapter between it and the
//! freedesktop thumbnailer contract: a `.thumbnailer` entry names an `Exec`
//! line, the file manager substitutes `%i` (input), `%o` (output PNG), and
//! `%s` (largest edge), and the exit code says whether a thumbnail happened.
//!
//! The thumbnail *cache* — `~/.cache/thumbnails`, the hashed names, the
//! `Thumb::URI` metadata — is deliberately not this program's business. The
//! calling thumbnail factory owns the cache and writes the metadata; helpers
//! that also write the cache race it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use peek_engine::Entry;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            // stderr reaches the journal of whichever file manager asked, which
            // is where someone debugging a missing thumbnail will look.
            eprintln!("peek-thumbnailer: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let [input, output, size] = args else {
        return Err("usage: peek-thumbnailer <input> <output.png> <size>".to_owned());
    };

    let size: u32 = size
        .parse()
        .map_err(|_| format!("size {size:?} is not a number"))?;
    // The spec's sizes are 128, 256, 512, and 1024; the clamp only refuses
    // values that could not be a thumbnail at all.
    let size = size.clamp(16, 2048);

    let path = local_path(input).ok_or_else(|| format!("{input:?} is not a local file"))?;
    let entry = Entry::load(&path).ok_or_else(|| format!("cannot read {}", path.display()))?;

    // The overlay's index sheet renders its cells through this same call, so
    // the icon in the file manager and the cell in the grid cannot disagree.
    let raster = peek_engine::thumbnail::render(&entry, size)
        .ok_or_else(|| format!("no thumbnail for {} ({})", path.display(), entry.mime))?;

    let image = image::RgbaImage::from_raw(raster.width, raster.height, raster.pixels)
        .ok_or_else(|| "the decoded raster was malformed".to_owned())?;

    image
        .save_with_format(Path::new(output), image::ImageFormat::Png)
        .map_err(|error| format!("could not write {output}: {error}"))?;

    Ok(())
}

/// Accept both the `%i` path form and the `%u` URI form of the Exec contract.
fn local_path(input: &str) -> Option<PathBuf> {
    if let Some(rest) = input.strip_prefix("file://") {
        let _ = rest;
        return url::Url::parse(input).ok()?.to_file_path().ok();
    }
    Some(PathBuf::from(input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_becomes_a_bounded_thumbnail() {
        let dir = std::env::temp_dir();
        let input = dir.join("peek-thumb-in.png");
        let output = dir.join("peek-thumb-out.png");

        let source = image::RgbaImage::from_pixel(600, 300, image::Rgba([10, 200, 10, 255]));
        source.save(&input).expect("write the fixture");

        run(&[
            input.display().to_string(),
            output.display().to_string(),
            "128".to_owned(),
        ])
        .expect("thumbnails");

        let thumb = image::open(&output).expect("reads back");
        assert_eq!(
            (thumb.width(), thumb.height()),
            (128, 64),
            "bounded by the longest edge, aspect kept"
        );

        let _ = std::fs::remove_file(input);
        let _ = std::fs::remove_file(output);
    }

    #[test]
    fn a_text_file_is_declined_rather_than_faked() {
        let dir = std::env::temp_dir();
        let input = dir.join("peek-thumb-notes.txt");
        let output = dir.join("peek-thumb-notes.png");
        std::fs::write(&input, "hello\n").expect("write");

        assert!(
            run(&[
                input.display().to_string(),
                output.display().to_string(),
                "128".to_owned(),
            ])
            .is_err(),
            "text has no thumbnail here"
        );
        assert!(!output.exists(), "a declined thumbnail writes nothing");

        let _ = std::fs::remove_file(input);
    }

    #[test]
    fn file_uris_are_accepted_alongside_paths() {
        assert_eq!(
            local_path("file:///tmp/a%20b.png"),
            Some(PathBuf::from("/tmp/a b.png"))
        );
        assert_eq!(
            local_path("/tmp/plain.png"),
            Some(PathBuf::from("/tmp/plain.png"))
        );
    }

    #[test]
    fn bad_arguments_fail_with_usage() {
        assert!(run(&["one".to_owned()]).is_err());
        assert!(
            run(&["a".to_owned(), "b".to_owned(), "large".to_owned()]).is_err(),
            "a non-numeric size is refused"
        );
    }
}
