// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Archive previews: what is inside, without unpacking it.
//!
//! Nothing here writes to disk. A previewer that extracts an archive to show you
//! its contents has done something the user did not ask for, in a directory they
//! did not choose, with a name collision they cannot see — and it has to clean up
//! afterwards, which is the part that goes wrong.
//!
//! Zip and 7z carry a central directory, so listing them is a seek and a parse
//! whatever the archive's size. Tar does not: its "directory" is the headers
//! interleaved with the data, so listing a `.tar.gz` means decompressing the
//! whole stream. That asymmetry is why [`MAX_ENTRIES`] exists — a tar of a
//! million small files would otherwise be read in full to build a list nobody
//! will scroll to the bottom of.

use std::io::Read;
use std::path::Path;

/// Entries listed before the previewer stops reading.
pub const MAX_ENTRIES: usize = 5_000;

/// Bytes of a compressed tar stream read before giving up on listing it.
///
/// Independent of [`MAX_ENTRIES`] because a tar can spend a gigabyte of stream
/// on a single file, reaching the byte ceiling long before the entry ceiling.
const MAX_TAR_BYTES: u64 = 256 * 1024 * 1024;

/// One member of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    /// Path as stored in the archive, separators and all.
    pub name: String,
    /// Uncompressed size. Zero for directories and for entries whose size the
    /// format does not record.
    pub size: u64,
    /// Stored size, when the format records one per entry. Tar has no
    /// per-member compression, so this is `None` there even for a `.tar.gz`.
    pub compressed: Option<u64>,
    pub is_dir: bool,
    /// Set when the member is stored encrypted, which is the reason a listing
    /// can succeed while extraction would need a password.
    pub encrypted: bool,
}

/// The contents of an archive.
#[derive(Debug, Clone, Default)]
pub struct Archive {
    pub members: Vec<Member>,
    /// Sum of the uncompressed sizes of the members listed.
    pub total_size: u64,
    /// Set when the archive holds more than was listed.
    pub truncated: bool,
    /// Human-facing format name, shown in the header.
    pub format: String,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("not an archive this previewer can read")]
    Unsupported,
    #[error("the archive is damaged: {0}")]
    Damaged(String),
}

/// List the contents of an archive.
///
/// `mime` comes from detection rather than being sniffed again here, because
/// the caller already has it and the two must not be able to disagree.
///
/// # Errors
///
/// Fails when the file cannot be read, is not a format this module handles, or
/// is damaged badly enough that no member could be read at all. A truncated
/// archive that yields *some* members is reported as a successful listing with
/// `truncated` set — showing what survived is more useful than showing nothing.
pub fn list(path: &Path, mime: &str) -> Result<Archive, Error> {
    match mime {
        "application/zip"
        | "application/x-zip-compressed"
        | "application/java-archive"
        | "application/vnd.android.package-archive" => zip(path, label(mime)),

        "application/x-7z-compressed" => seven_z(path),

        "application/x-tar" => tar_stream(std::fs::File::open(path).map_err(io(path))?, "Tar"),

        "application/x-compressed-tar" | "application/gzip" => {
            let file = std::fs::File::open(path).map_err(io(path))?;
            tar_stream(flate2::read::GzDecoder::new(file), "Tar (gzip)")
        }

        "application/x-bzip-compressed-tar"
        | "application/x-bzip2-compressed-tar"
        | "application/x-bzip"
        | "application/x-bzip2" => {
            let file = std::fs::File::open(path).map_err(io(path))?;
            tar_stream(bzip2::read::BzDecoder::new(file), "Tar (bzip2)")
        }

        "application/x-xz-compressed-tar"
        | "application/x-xz"
        | "application/x-lzma-compressed-tar" => {
            let file = std::fs::File::open(path).map_err(io(path))?;
            tar_stream(xz2::read::XzDecoder::new(file), "Tar (xz)")
        }

        "application/x-zstd-compressed-tar" | "application/zstd" => {
            let file = std::fs::File::open(path).map_err(io(path))?;
            let decoder = zstd::stream::read::Decoder::new(file).map_err(io(path))?;
            tar_stream(decoder, "Tar (zstd)")
        }

        _ => Err(Error::Unsupported),
    }
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |source| Error::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Display name for the zip family, which shares one container.
fn label(mime: &str) -> &'static str {
    match mime {
        "application/java-archive" => "Java archive",
        "application/vnd.android.package-archive" => "Android package",
        _ => "Zip",
    }
}

fn zip(path: &Path, format: &str) -> Result<Archive, Error> {
    let file = std::fs::File::open(path).map_err(io(path))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| Error::Damaged(error.to_string()))?;

    let count = archive.len();
    let mut members = Vec::with_capacity(count.min(MAX_ENTRIES));
    let mut total_size = 0;

    for index in 0..count.min(MAX_ENTRIES) {
        // An encrypted member's *metadata* is still readable, so a listing
        // works where extraction would not. `by_index_raw` avoids setting up a
        // decompressor we are never going to read from.
        let Ok(entry) = archive.by_index_raw(index) else {
            continue;
        };

        let size = entry.size();
        total_size += size;
        members.push(Member {
            name: entry.name().to_owned(),
            size,
            compressed: Some(entry.compressed_size()),
            is_dir: entry.is_dir(),
            encrypted: entry.encrypted(),
        });
    }

    Ok(Archive {
        members,
        total_size,
        truncated: count > MAX_ENTRIES,
        format: format.to_owned(),
    })
}

fn seven_z(path: &Path) -> Result<Archive, Error> {
    let reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(|error| Error::Damaged(error.to_string()))?;

    let files = &reader.archive().files;
    let mut members = Vec::with_capacity(files.len().min(MAX_ENTRIES));
    let mut total_size = 0;

    for entry in files.iter().take(MAX_ENTRIES) {
        total_size += entry.size;
        members.push(Member {
            name: entry.name.clone(),
            size: entry.size,
            // 7z compresses members together in solid blocks, so there is no
            // meaningful per-entry compressed size to report.
            compressed: None,
            is_dir: entry.is_directory,
            encrypted: false,
        });
    }

    Ok(Archive {
        members,
        total_size,
        truncated: files.len() > MAX_ENTRIES,
        format: "7z".to_owned(),
    })
}

/// List a tar stream, however it is compressed.
///
/// Generic over the reader so the four compressed variants share one
/// implementation; the only thing that differs between them is the decoder the
/// bytes come through.
fn tar_stream<R: Read>(reader: R, format: &str) -> Result<Archive, Error> {
    // Bounded at the source. A tar's listing requires walking the stream, so
    // without this a `.tar.zst` of a video library would be decompressed in
    // full to produce a list.
    let mut archive = tar::Archive::new(reader.take(MAX_TAR_BYTES));

    let entries = archive
        .entries()
        .map_err(|error| Error::Damaged(error.to_string()))?;

    let mut members = Vec::new();
    let mut total_size = 0;
    let mut truncated = false;
    let mut damage: Option<String> = None;

    for entry in entries {
        if members.len() >= MAX_ENTRIES {
            truncated = true;
            break;
        }

        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                // Hitting the byte ceiling surfaces as a truncated read, which
                // is indistinguishable from real damage at this layer. Either
                // way the members read so far are worth showing.
                damage = Some(error.to_string());
                truncated = true;
                break;
            }
        };

        let header = entry.header();
        let size = header.size().unwrap_or(0);
        total_size += size;

        members.push(Member {
            name: entry
                .path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| "?".to_owned()),
            size,
            compressed: None,
            is_dir: header.entry_type().is_dir(),
            encrypted: false,
        });
    }

    // Only a listing that produced nothing at all is a failure; anything else
    // is a partial result the user can still read.
    if members.is_empty()
        && let Some(damage) = damage
    {
        return Err(Error::Damaged(damage));
    }

    Ok(Archive {
        members,
        total_size,
        truncated,
        format: format.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_formats_are_declined_rather_than_guessed_at() {
        let path = std::env::temp_dir().join("peek-test-archive.bin");
        std::fs::write(&path, b"nope").expect("write");

        assert!(matches!(
            list(&path, "application/octet-stream"),
            Err(Error::Unsupported)
        ));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_damaged_zip_is_reported_not_panicked_on() {
        let path = std::env::temp_dir().join("peek-test-broken.zip");
        std::fs::write(&path, b"PK\x03\x04 truncated").expect("write");

        assert!(matches!(
            list(&path, "application/zip"),
            Err(Error::Damaged(_))
        ));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_empty_tar_lists_as_empty() {
        // Two zero blocks are a valid, empty tar.
        let archive = tar_stream(std::io::Cursor::new(vec![0u8; 1024]), "Tar").expect("lists");
        assert!(archive.members.is_empty());
        assert!(!archive.truncated);
    }

    #[test]
    fn zip_family_names_are_distinguished() {
        assert_eq!(label("application/java-archive"), "Java archive");
        assert_eq!(label("application/zip"), "Zip");
    }
}
