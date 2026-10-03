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

/// Memory an xz or LZMA decoder may allocate, in KiB.
///
/// The dictionary size is the stream's own claim, and a header can claim more
/// than a gigabyte. `xz -9` needs 64 MiB to decode; four times that covers
/// every preset and refuses a header that exists only to allocate.
const DECODER_MEMORY_KIB: u32 = 256 * 1024;

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

        "application/x-tar" => tar_stream(open(path)?, "Tar"),

        // The `*-compressed-tar` types name a tarball outright. The bare
        // compression types are what magic sniffing answers for *any* file
        // in that compression — it cannot tell `backup.tar.gz` from
        // `access.log.gz` — so those are listed only when what they hold
        // turns out to be a tar. See [`tar_if_tar`].
        "application/x-compressed-tar" => tar_stream(gzip(path)?, GZIP),
        "application/gzip" => tar_if_tar(gzip(path)?, GZIP),

        "application/x-bzip-compressed-tar" | "application/x-bzip2-compressed-tar" => {
            tar_stream(bzip2(path)?, BZIP2)
        }
        "application/x-bzip" | "application/x-bzip2" => tar_if_tar(bzip2(path)?, BZIP2),

        "application/x-xz-compressed-tar" => tar_stream(xz(path)?, XZ),
        "application/x-xz" => tar_if_tar(xz(path)?, XZ),

        // `.tar.lzma` is the older LZMA-alone container, not xz: same codec,
        // a different header, and an xz decoder rejects it.
        "application/x-lzma-compressed-tar" => tar_stream(lzma(path)?, "Tar (lzma)"),

        "application/x-zstd-compressed-tar" => tar_stream(zstd(path)?, ZSTD),
        "application/zstd" => tar_if_tar(zstd(path)?, ZSTD),

        _ => Err(Error::Unsupported),
    }
}

const GZIP: &str = "Tar (gzip)";
const BZIP2: &str = "Tar (bzip2)";
const XZ: &str = "Tar (xz)";
const ZSTD: &str = "Tar (zstd)";

fn open(path: &Path) -> Result<std::fs::File, Error> {
    std::fs::File::open(path).map_err(io(path))
}

fn gzip(path: &Path) -> Result<impl Read, Error> {
    Ok(flate2::read::GzDecoder::new(open(path)?))
}

fn bzip2(path: &Path) -> Result<impl Read, Error> {
    Ok(bzip2::read::BzDecoder::new(open(path)?))
}

fn xz(path: &Path) -> Result<impl Read, Error> {
    Ok(lzma_rust2::XzReader::new_mem_limit(
        open(path)?,
        true,
        DECODER_MEMORY_KIB,
    ))
}

fn lzma(path: &Path) -> Result<impl Read, Error> {
    lzma_rust2::LzmaReader::new_mem_limit(open(path)?, DECODER_MEMORY_KIB, None)
        .map_err(|error| Error::Damaged(error.to_string()))
}

fn zstd(path: &Path) -> Result<impl Read, Error> {
    zstd::stream::read::Decoder::new(open(path)?).map_err(io(path))
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

/// List a compressed stream if it holds a tar, and decline it otherwise.
///
/// Only the first block is decompressed to decide: a tar starts with a
/// header whose checksum covers the block, which a log, a disk image or a
/// database dump will not satisfy by accident. Declining is
/// [`Error::Unsupported`], not damage — a gzipped log is a perfectly good
/// file that simply is not an archive.
fn tar_if_tar<R: Read>(mut reader: R, format: &str) -> Result<Archive, Error> {
    let mut head = Vec::with_capacity(TAR_BLOCK);
    (&mut reader)
        .take(TAR_BLOCK as u64)
        .read_to_end(&mut head)
        .map_err(|error| Error::Damaged(error.to_string()))?;

    if !is_tar_header(&head) {
        return Err(Error::Unsupported);
    }
    tar_stream(std::io::Cursor::new(head).chain(reader), format)
}

/// Size of a tar header block.
const TAR_BLOCK: usize = 512;

/// Whether a block is a tar header: its stored checksum matches the sum of
/// its bytes with the checksum field read as spaces. An all-zero block is the
/// end-of-archive marker, which is how an empty tar begins.
fn is_tar_header(block: &[u8]) -> bool {
    let Some(block) = block.get(..TAR_BLOCK) else {
        return false;
    };
    if block.iter().all(|&byte| byte == 0) {
        return true;
    }

    let Ok(stored) = tar::Header::from_byte_slice(block).cksum() else {
        return false;
    };
    let computed: u32 = block
        .iter()
        .enumerate()
        .map(|(index, &byte)| {
            if (148..156).contains(&index) {
                u32::from(b' ')
            } else {
                u32::from(byte)
            }
        })
        .sum();
    stored == computed
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
        assert_eq!(archive.members, []);
        assert!(!archive.truncated);
    }

    fn gzipped(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut encoder, bytes).expect("compress");
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, encoder.finish().expect("finish")).expect("write");
        path
    }

    #[test]
    fn a_compressed_file_that_is_not_a_tar_is_declined_not_damaged() {
        // Magic says `application/gzip` for any gzip, so a compressed log
        // arrives here too. It is not an archive, and it is not broken.
        let path = gzipped(
            "peek-test-archive-log.gz",
            &b"GET / HTTP/1.1 200\n".repeat(64),
        );
        assert!(matches!(
            list(&path, "application/gzip"),
            Err(Error::Unsupported)
        ));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_bare_gzip_holding_a_tar_is_listed() {
        let mut tarball = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(2);
        header.set_mode(0o644);
        header.set_cksum();
        tarball
            .append_data(&mut header, "a.txt", &b"hi"[..])
            .expect("append");
        let path = gzipped(
            "peek-test-archive-bare.gz",
            &tarball.into_inner().expect("finish the tar"),
        );

        let archive = list(&path, "application/gzip").expect("lists");
        assert_eq!(archive.members.len(), 1);
        assert_eq!(archive.members[0].name, "a.txt");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_empty_compressed_tar_is_still_a_tar() {
        let path = gzipped("peek-test-archive-empty.tar.gz", &[0u8; 1024]);
        let archive = list(&path, "application/gzip").expect("lists");
        assert_eq!(archive.members, []);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_xz_tar_lists_through_the_rust_decoder() {
        let mut tarball = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        tarball
            .append_data(&mut header, "notes.txt", &b"hello"[..])
            .expect("append");
        let tarball = tarball.into_inner().expect("finish the tar");

        let mut xz = lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(6))
            .expect("encoder");
        std::io::Write::write_all(&mut xz, &tarball).expect("compress");
        let compressed = xz.finish().expect("finish the xz stream");

        let path = std::env::temp_dir().join("peek-test-archive.tar.xz");
        std::fs::write(&path, compressed).expect("write");

        let archive = list(&path, "application/x-xz-compressed-tar").expect("lists");
        assert_eq!(archive.format, "Tar (xz)");
        assert_eq!(archive.members.len(), 1);
        assert_eq!(archive.members[0].name, "notes.txt");
        assert_eq!(archive.members[0].size, 5);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_tar_lzma_lists_through_the_lzma_alone_decoder() {
        // `.tar.lzma` is LZMA-alone, not xz; an xz decoder rejects it.
        let mut tarball = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(3);
        header.set_mode(0o644);
        header.set_cksum();
        tarball
            .append_data(&mut header, "old.txt", &b"abc"[..])
            .expect("append");
        let tarball = tarball.into_inner().expect("finish the tar");

        let mut lzma = lzma_rust2::LzmaWriter::new_use_header(
            Vec::new(),
            &lzma_rust2::LzmaOptions::with_preset(6),
            Some(tarball.len() as u64),
        )
        .expect("encoder");
        std::io::Write::write_all(&mut lzma, &tarball).expect("compress");
        let path = std::env::temp_dir().join("peek-test-archive.tar.lzma");
        std::fs::write(&path, lzma.finish().expect("finish")).expect("write");

        let archive = list(&path, "application/x-lzma-compressed-tar").expect("lists");
        assert_eq!(archive.members.len(), 1);
        assert_eq!(archive.members[0].name, "old.txt");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_7z_lists_its_members() {
        let path = std::env::temp_dir().join("peek-test-archive.7z");
        {
            let mut writer = sevenz_rust2::ArchiveWriter::create(&path).expect("create");
            writer
                .push_archive_entry::<&[u8]>(
                    sevenz_rust2::ArchiveEntry::new_directory("docs"),
                    None,
                )
                .expect("directory");
            writer
                .push_archive_entry(
                    sevenz_rust2::ArchiveEntry::new_file("docs/readme.txt"),
                    Some(&b"seven bytes"[..7]),
                )
                .expect("file");
            writer.finish().expect("finish");
        }

        let archive = list(&path, "application/x-7z-compressed").expect("lists");
        assert_eq!(archive.format, "7z");
        assert_eq!(archive.members.len(), 2);
        let file = archive
            .members
            .iter()
            .find(|member| member.name == "docs/readme.txt")
            .expect("the file is listed");
        assert_eq!(file.size, 7);
        assert!(!file.is_dir);
        assert!(archive.members.iter().any(|member| member.is_dir));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn zip_family_names_are_distinguished() {
        assert_eq!(label("application/java-archive"), "Java archive");
        assert_eq!(label("application/zip"), "Zip");
    }
}
