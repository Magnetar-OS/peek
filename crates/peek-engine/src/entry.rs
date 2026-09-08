// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! The file under the cursor, and the files either side of it.
//!
//! QuickLook's arrow keys move through the *selection* — which is something only
//! the file manager knows. `peek` gets told about one file at a time, so it
//! reconstructs a neighbourhood by listing the containing directory. That is not
//! a workaround for a missing API: when the file manager does drive the
//! navigation (Nautilus, over `SelectionEvent`), the neighbourhood is replaced by
//! whatever it selects next, and when nothing drives it — a file opened from a
//! terminal, or one of several passed on the command line — arrow keys still do
//! the useful thing.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::kind::{self, Kind};

/// A file, resolved far enough to decide how to preview it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// File name as displayed. Falls back to the whole path for the root and
    /// for paths ending in `..`, neither of which has a file name.
    pub name: String,
    pub mime: String,
    pub kind: Kind,
    /// Size in bytes. Zero for directories: the recursive size is expensive
    /// enough that the directory previewer computes it separately and shows a
    /// running total.
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Target of a symlink, when this entry is one. Shown on the metadata card,
    /// because "why is this file 47 bytes" usually has this as the answer.
    pub link_target: Option<PathBuf>,
    /// Unix mode bits, for the permissions row.
    pub mode: u32,
}

impl Entry {
    /// Resolve a path into an entry.
    ///
    /// Returns `None` only when the path cannot be stat'd at all — a file that
    /// was deleted between the file manager selecting it and the previewer
    /// being asked for it, which happens often enough to be worth handling
    /// rather than surfacing as an error.
    #[must_use]
    pub fn load(path: &Path) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;

        // `symlink_metadata` first so a broken symlink is describable rather
        // than simply missing; the followed metadata is what sizes it.
        let link = std::fs::symlink_metadata(path).ok()?;
        let link_target = if link.file_type().is_symlink() {
            std::fs::read_link(path).ok()
        } else {
            None
        };

        let meta = std::fs::metadata(path).ok().unwrap_or(link);
        let (mime, kind) = kind::detect(path);

        Some(Self {
            name: path.file_name().map_or_else(
                || path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            ),
            mime,
            kind,
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: meta.modified().ok(),
            link_target,
            mode: meta.mode(),
            path: path.to_path_buf(),
        })
    }

    /// Whether the file is hidden by the desktop's convention.
    #[must_use]
    pub fn is_hidden(&self) -> bool {
        self.name.starts_with('.')
    }
}

/// A file and the files it can be stepped to.
///
/// The current file is always present at `index`, even when it does not appear
/// in the directory listing — a file that was just deleted, or one reached
/// through a symlink from elsewhere, must still be previewable.
#[derive(Debug, Clone, Default)]
pub struct Neighbourhood {
    paths: Vec<PathBuf>,
    index: usize,
}

impl Neighbourhood {
    /// Build a neighbourhood from an explicit list, positioned at `index`.
    ///
    /// This is the shape `peek a.png b.png c.png` produces: the user named the
    /// set, so the directory is not consulted.
    #[must_use]
    pub fn from_list(paths: Vec<PathBuf>, index: usize) -> Self {
        let index = index.min(paths.len().saturating_sub(1));
        Self { paths, index }
    }

    /// Build a neighbourhood by listing the directory containing `path`.
    ///
    /// Hidden files are included only when the file being previewed is itself
    /// hidden. Previewing `.bashrc` and then finding the arrow keys will not
    /// reach `.profile` would be surprising; previewing a photo and having the
    /// arrows walk into `.thumbnails` would be worse.
    #[must_use]
    pub fn around(path: &Path) -> Self {
        let single = || Self {
            paths: vec![path.to_path_buf()],
            index: 0,
        };

        let Some(parent) = path.parent() else {
            return single();
        };
        let Ok(dir) = std::fs::read_dir(parent) else {
            return single();
        };

        let include_hidden = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'));

        let mut paths: Vec<PathBuf> = dir
            .filter_map(Result::ok)
            .filter(|entry| {
                // Directories are skipped: stepping into one mid-preview would
                // silently change which directory the arrows are walking, and
                // there is no way back with two keys.
                !entry.file_type().is_ok_and(|kind| kind.is_dir())
            })
            .map(|entry| entry.path())
            .filter(|candidate| {
                include_hidden
                    || candidate
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| !name.starts_with('.'))
            })
            .collect();

        // Natural order, so `img2` sorts before `img10` the way it does in the
        // file manager the user is looking at.
        paths.sort_by(|a, b| natural(&name_of(a), &name_of(b)));

        let index = paths.iter().position(|candidate| candidate == path);
        match index {
            Some(index) => Self { paths, index },
            // The file is not in its own directory listing — it was deleted, or
            // the filter excluded it. Preview it alone rather than jumping to
            // an unrelated neighbour.
            None => single(),
        }
    }

    /// The file currently being previewed.
    #[must_use]
    pub fn current(&self) -> Option<&Path> {
        self.paths.get(self.index).map(PathBuf::as_path)
    }

    /// Move by a signed offset, wrapping at both ends.
    ///
    /// Wrapping rather than clamping: the neighbourhood is a ring the user is
    /// flicking through, and stopping dead at the last photo in a directory
    /// reads as the previewer having failed rather than as having arrived.
    /// Returns whether the position actually changed.
    pub fn step(&mut self, delta: isize) -> bool {
        let count = self.paths.len();
        if count <= 1 {
            return false;
        }

        let count = count as isize;
        let index = (self.index as isize + delta).rem_euclid(count);
        let moved = index as usize != self.index;
        self.index = index as usize;
        moved
    }

    /// Jump to a specific path, keeping the rest of the neighbourhood.
    ///
    /// Used when the file manager drives navigation: it has already changed the
    /// selection, so the previewer follows rather than deciding.
    pub fn focus(&mut self, path: &Path) {
        match self.paths.iter().position(|candidate| candidate == path) {
            Some(index) => self.index = index,
            None => {
                // Selected something outside the listing — a different
                // directory, or a file created since. Re-derive rather than
                // appending, so position and ordering stay honest.
                *self = Self::around(path);
            }
        }
    }

    /// Position within the neighbourhood, as `(current, total)`, one-based.
    #[must_use]
    pub fn position(&self) -> (usize, usize) {
        (self.index + 1, self.paths.len())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Every path in the neighbourhood, in display order.
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Compare two file names the way a file manager does: digit runs compare as
/// numbers, everything else compares case-insensitively.
///
/// `pub(crate)` because comic pages need the same order: `page2` before
/// `page10`, exactly as the archive's author counted them.
pub(crate) fn natural(a: &str, b: &str) -> Ordering {
    let mut left = a.chars().peekable();
    let mut right = b.chars().peekable();

    loop {
        match (left.peek().copied(), right.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let x = take_number(&mut left);
                let y = take_number(&mut right);
                match x.cmp(&y) {
                    Ordering::Equal => {}
                    ordering => return ordering,
                }
            }
            (Some(x), Some(y)) => {
                left.next();
                right.next();
                match x.to_lowercase().cmp(y.to_lowercase()) {
                    Ordering::Equal => {}
                    ordering => return ordering,
                }
            }
        }
    }
}

/// Consume a run of digits and return its value.
///
/// Saturating rather than wrapping: a file named with a 30-digit number is not
/// worth a panic, and any two such files comparing equal is the right outcome
/// for a sort.
fn take_number(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> u128 {
    let mut value: u128 = 0;
    while let Some(digit) = chars.peek().and_then(|c| c.to_digit(10)) {
        value = value.saturating_mul(10).saturating_add(u128::from(digit));
        chars.next();
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_sort_numerically() {
        assert_eq!(natural("img2.png", "img10.png"), Ordering::Less);
        assert_eq!(natural("img10.png", "img2.png"), Ordering::Greater);
    }

    #[test]
    fn names_sort_case_insensitively() {
        assert_eq!(natural("Alpha", "beta"), Ordering::Less);
    }

    #[test]
    fn identical_names_are_equal_by_the_final_tiebreak() {
        assert_eq!(natural("same", "same"), Ordering::Equal);
    }

    #[test]
    fn stepping_wraps_at_both_ends() {
        let mut hood = Neighbourhood::from_list(vec!["a".into(), "b".into(), "c".into()], 0);

        assert!(hood.step(-1));
        assert_eq!(hood.current(), Some(Path::new("c")));
        assert!(hood.step(1));
        assert_eq!(hood.current(), Some(Path::new("a")));
    }

    #[test]
    fn a_lone_file_does_not_step() {
        let mut hood = Neighbourhood::from_list(vec!["only".into()], 0);
        assert!(!hood.step(1));
        assert_eq!(hood.position(), (1, 1));
    }

    #[test]
    fn focusing_a_known_path_keeps_the_neighbourhood() {
        let mut hood = Neighbourhood::from_list(vec!["a".into(), "b".into(), "c".into()], 0);
        hood.focus(Path::new("c"));
        assert_eq!(hood.position(), (3, 3));
    }
}
