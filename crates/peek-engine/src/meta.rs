// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! The metadata card and the directory summary.
//!
//! The card is the fallback previewer: whatever the file turns out to be, it can
//! always be described. That makes it the one previewer that must never fail —
//! an unreadable field is omitted rather than turned into an error, because the
//! alternative is an empty overlay for a file the user can plainly see exists.
//!
//! Directories get their own summary rather than the card, because the useful
//! facts about a directory are all about its contents and none of them are in
//! its `stat`.

use std::path::Path;
use std::time::{Duration, SystemTime};

use humansize::{DECIMAL, format_size};

use crate::entry::Entry;

/// Entries examined when summarising a directory.
///
/// A home directory's `node_modules` has hundreds of thousands of files, and
/// walking it to show a count on a preview panel is work with no reader.
pub const MAX_SCAN: usize = 20_000;

/// Which fact a card row states.
///
/// An enum rather than a label, because the label is prose and prose belongs to
/// whichever frontend is drawing it — the engine knows *that* a file has a
/// modification time, not what the word for it is in the user's language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Kind,
    Type,
    Size,
    Modified,
    Permissions,
    SymlinkTo,
    Where,
}

/// What a card row states, in a form the frontend can render in any language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    /// Already language-neutral: a MIME type, a path, a permissions string.
    Text(String),
    /// A byte count. Formatting it is a locale decision, so it crosses the
    /// boundary as a number.
    Bytes(u64),
    /// How long ago something happened, bucketed but not worded.
    Elapsed(Elapsed),
    /// A MIME type to be described in the reader's language — "PNG image" is
    /// a sentence, and sentences belong to the frontend.
    Mime(String),
}

/// How long ago a timestamp was, in the buckets the card distinguishes.
///
/// The arithmetic — including the calendar conversion — is the same in every
/// language, so it stays here. Only the words move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Elapsed {
    JustNow,
    Minutes(u64),
    Hours(u64),
    Days(u64),
    /// `YYYY-MM-DD HH:MM` in UTC, for anything older than a week or dated in
    /// the future. Digits and separators, so there is nothing to translate.
    On(String),
    /// A timestamp before the Unix epoch.
    BeforeEpoch,
}

/// A row on the metadata card.
pub type Row = (Field, Value);

/// What the fallback previewer shows.
#[derive(Debug, Clone, Default)]
pub struct Card {
    pub rows: Vec<Row>,
    /// Icon-theme name for the file's type, so the card shows the same icon the
    /// file manager does rather than a generic document.
    pub icon: String,
}

/// What a directory contains.
#[derive(Debug, Clone, Default)]
pub struct Directory {
    pub files: usize,
    pub directories: usize,
    /// Total size of the files counted. Not recursive: descending into a
    /// directory tree to size it is a `du`, not a preview.
    pub size: u64,
    /// Set when the directory holds more entries than were scanned.
    pub truncated: bool,
    /// A few names, so the panel shows what kind of directory this is rather
    /// than only how big it is.
    pub sample: Vec<String>,
    /// Set when the directory could not be read at all — a permissions
    /// problem, almost always.
    pub unreadable: bool,
}

/// Names shown in the directory sample.
const SAMPLE_SIZE: usize = 12;

/// Build the metadata card for a file.
///
/// Never fails. Every field is optional and a field that cannot be read is
/// simply absent.
#[must_use]
pub fn card(entry: &Entry) -> Card {
    let mut rows = vec![
        (Field::Kind, Value::Mime(entry.mime.clone())),
        (Field::Type, Value::Text(entry.mime.clone())),
    ];

    if !matches!(entry.kind, crate::kind::Kind::Directory) {
        rows.push((Field::Size, Value::Bytes(entry.size)));
    }

    if let Some(modified) = entry.modified {
        rows.push((Field::Modified, Value::Elapsed(elapsed(modified))));
    }

    rows.push((Field::Permissions, Value::Text(permissions(entry.mode))));

    if let Some(target) = &entry.link_target {
        rows.push((Field::SymlinkTo, Value::Text(target.display().to_string())));
    }

    rows.push((
        Field::Where,
        Value::Text(
            entry
                .path
                .parent()
                .map_or_else(|| "/".to_owned(), |parent| parent.display().to_string()),
        ),
    ));

    Card {
        rows,
        icon: icon_name(&entry.mime),
    }
}

/// Hash a file, for the card's checksum row.
///
/// Separate from [`card`] and never called automatically: hashing reads the
/// whole file, and doing that for every file the user arrows past would turn a
/// previewer into a disk-thrashing background job. The overlay asks for it on
/// request.
///
/// # Errors
///
/// Fails when the file cannot be read.
pub fn checksum(path: &Path) -> std::io::Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    // A fixed buffer rather than reading to a `Vec`: this has to work on a file
    // larger than memory, which is exactly the kind of file someone wants a
    // checksum of.
    let mut buffer = vec![0u8; 128 * 1024];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher.finalize().to_hex().to_string())
}

/// Summarise a directory's immediate contents.
#[must_use]
pub fn directory(path: &Path) -> Directory {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Directory {
            unreadable: true,
            ..Directory::default()
        };
    };

    let mut summary = Directory::default();

    for (scanned, entry) in entries.filter_map(Result::ok).enumerate() {
        if scanned >= MAX_SCAN {
            summary.truncated = true;
            break;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        let hidden = name.starts_with('.');

        // `metadata` here follows symlinks, which is what a file manager shows:
        // a symlink to a directory is presented as a directory.
        let is_dir = entry
            .metadata()
            .map(|meta| meta.is_dir())
            .unwrap_or_else(|_| entry.file_type().is_ok_and(|kind| kind.is_dir()));

        if is_dir {
            summary.directories += 1;
        } else {
            summary.files += 1;
            summary.size += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
        }

        if !hidden && summary.sample.len() < SAMPLE_SIZE {
            summary.sample.push(name);
        }
    }

    summary.sample.sort();
    summary
}

/// The displayable core of a MIME subtype.
///
/// `image/x-pixel-art` yields `PIXEL-ART`: the vendor and experimental
/// prefixes carry no meaning for a reader, and the uppercased remainder is at
/// least honest about what the type calls itself. The frontend combines this
/// with a worded category — "{subtype} image" — in the user's language.
#[must_use]
pub fn mime_subtype(mime: &str) -> String {
    let subtype = mime.split_once('/').map_or(mime, |(_, subtype)| subtype);
    subtype
        .trim_start_matches("x-")
        .trim_start_matches("vnd.")
        .to_uppercase()
}

/// Icon-theme name for a MIME type.
///
/// The freedesktop convention is the MIME type with `/` replaced by `-`, with a
/// generic fallback per category. Icon themes are patchy about which specific
/// types they cover, so both are offered and the frontend takes the first that
/// resolves.
#[must_use]
pub fn icon_name(mime: &str) -> String {
    mime.replace('/', "-")
}

/// Generic icon for a MIME type's category, used when the specific one is
/// missing from the icon theme.
#[must_use]
pub fn generic_icon_name(mime: &str) -> &'static str {
    match mime.split('/').next().unwrap_or_default() {
        "image" => "image-x-generic",
        "video" => "video-x-generic",
        "audio" => "audio-x-generic",
        "text" => "text-x-generic",
        "font" => "font-x-generic",
        "inode" => "folder",
        _ => "application-x-generic",
    }
}

/// Format Unix mode bits as `rwxr-xr-x (755)`.
#[must_use]
pub fn permissions(mode: u32) -> String {
    let bit = |shift: u32, flag: u32, symbol: char| {
        if (mode >> shift) & flag == flag {
            symbol
        } else {
            '-'
        }
    };

    let symbolic: String = [
        bit(6, 4, 'r'),
        bit(6, 2, 'w'),
        bit(6, 1, 'x'),
        bit(3, 4, 'r'),
        bit(3, 2, 'w'),
        bit(3, 1, 'x'),
        bit(0, 4, 'r'),
        bit(0, 2, 'w'),
        bit(0, 1, 'x'),
    ]
    .into_iter()
    .collect();

    format!("{symbolic} ({:03o})", mode & 0o777)
}

/// Format a timestamp as an age.
///
/// "3 hours ago" rather than a date, because the question a previewer's
/// modified row answers is almost always "is this the one I just saved".
/// Anything older than a week gets an absolute date instead, where the age
/// stops being the useful framing.
#[must_use]
pub fn elapsed(time: SystemTime) -> Elapsed {
    let Ok(since) = time.elapsed() else {
        // A file with a timestamp in the future — a bad clock, or an archive
        // extracted with one. Reporting the absolute time is the only honest
        // answer.
        return absolute_time(time);
    };

    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    const WEEK: u64 = 7 * DAY;

    let seconds = since.as_secs();
    match seconds {
        0..MINUTE => Elapsed::JustNow,
        MINUTE..HOUR => Elapsed::Minutes(seconds / MINUTE),
        HOUR..DAY => Elapsed::Hours(seconds / HOUR),
        DAY..WEEK => Elapsed::Days(seconds / DAY),
        _ => absolute_time(time),
    }
}

/// Format a timestamp as `YYYY-MM-DD HH:MM`, in UTC.
///
/// UTC rather than local time because pulling in a timezone database for one
/// row on a preview panel is not a trade worth making, and the frontend labels
/// the row as UTC so it is not misleading.
fn absolute_time(time: SystemTime) -> Elapsed {
    let Ok(since_epoch) = time.duration_since(SystemTime::UNIX_EPOCH) else {
        return Elapsed::BeforeEpoch;
    };
    Elapsed::On(civil_from_unix(since_epoch))
}

/// Convert a Unix timestamp to a calendar date and time.
///
/// Howard Hinnant's `civil_from_days`, which is the standard branch-free
/// algorithm for this and handles leap years and centuries without a table.
fn civil_from_unix(since_epoch: Duration) -> String {
    let secs = since_epoch.as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);

    // Shift the epoch to 0000-03-01 so leap days land at the end of the cycle.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 { year + 1 } else { year };

    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        time_of_day / 3600,
        (time_of_day % 3600) / 60
    )
}

/// Insert thin separators into a number: `1073741824` → `1 073 741 824`.
///
/// Spaces rather than commas or periods, which mean opposite things in
/// different locales.
#[must_use]
pub fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);

    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('\u{202f}');
        }
        grouped.push(digit);
    }

    grouped
}

/// Format a duration as `M:SS` or `H:MM:SS`.
#[must_use]
pub fn duration(value: Duration) -> String {
    let total = value.as_secs();
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Format a byte count for display.
#[must_use]
pub fn size(bytes: u64) -> String {
    format_size(bytes, DECIMAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_render_symbolically_and_octally() {
        assert_eq!(permissions(0o100_644), "rw-r--r-- (644)");
        assert_eq!(permissions(0o040_755), "rwxr-xr-x (755)");
        assert_eq!(permissions(0o000), "--------- (000)");
    }

    #[test]
    fn recent_times_are_relative() {
        let recent = SystemTime::now() - Duration::from_secs(3 * 3600);
        assert_eq!(elapsed(recent), Elapsed::Hours(3));

        let moment = SystemTime::now() - Duration::from_secs(5);
        assert_eq!(elapsed(moment), Elapsed::JustNow);
    }

    #[test]
    fn old_times_are_absolute() {
        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        assert_eq!(elapsed(old), Elapsed::On("2001-09-09 01:46".to_owned()));
    }

    #[test]
    fn a_pre_epoch_timestamp_is_reported_rather_than_wrapped() {
        let ancient = SystemTime::UNIX_EPOCH - Duration::from_secs(86_400);
        assert_eq!(elapsed(ancient), Elapsed::BeforeEpoch);
    }

    #[test]
    fn the_epoch_converts_correctly() {
        assert_eq!(civil_from_unix(Duration::ZERO), "1970-01-01 00:00");
    }

    #[test]
    fn a_leap_day_converts_correctly() {
        // 2024-02-29T12:00:00Z
        assert_eq!(
            civil_from_unix(Duration::from_secs(1_709_208_000)),
            "2024-02-29 12:00"
        );
    }

    #[test]
    fn digits_are_grouped_in_threes() {
        assert_eq!(group_digits(1), "1");
        assert_eq!(group_digits(1000), "1\u{202f}000");
        assert_eq!(
            group_digits(1_073_741_824),
            "1\u{202f}073\u{202f}741\u{202f}824"
        );
    }

    #[test]
    fn durations_gain_an_hours_field_only_when_needed() {
        assert_eq!(duration(Duration::from_secs(65)), "1:05");
        assert_eq!(duration(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn mime_subtypes_shed_their_vendor_prefixes() {
        assert_eq!(mime_subtype("image/png"), "PNG");
        assert_eq!(mime_subtype("image/x-tga"), "TGA");
        assert_eq!(mime_subtype("application/vnd.rar"), "RAR");
        assert_eq!(mime_subtype("no-slash"), "NO-SLASH");
    }

    #[test]
    fn icon_names_follow_the_freedesktop_convention() {
        assert_eq!(icon_name("image/png"), "image-png");
        assert_eq!(generic_icon_name("image/png"), "image-x-generic");
        assert_eq!(
            generic_icon_name("application/pdf"),
            "application-x-generic"
        );
    }
}
