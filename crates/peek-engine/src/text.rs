// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Text and source previews.
//!
//! Highlighting is done here rather than in the frontend because it is the
//! expensive part — parsing a syntax definition against a few thousand lines —
//! and it must not happen on the frame that draws them. The output is a plain
//! list of coloured spans, which any toolkit can render without knowing that
//! syntect exists.
//!
//! Two limits apply, both about latency rather than memory. A previewer is
//! opened on whatever the cursor happens to be sitting on, and that is
//! sometimes a 400 MB log file; reading all of it to show the first screenful
//! would be work done entirely for nothing.

use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Bytes read from the head of a file.
///
/// Four megabytes is far more than fits on a screen and still small enough to
/// read and highlight inside one frame's worth of budget on a background
/// thread.
pub const MAX_BYTES: usize = 4 * 1024 * 1024;

/// Lines kept before the document is reported as truncated.
///
/// A memory bound, not a latency one: the frontend shapes only the lines near
/// the scroll position, so line count no longer costs anything per frame. What
/// remains is the `Line` structures themselves — a pathological file of bare
/// newlines inside [`MAX_BYTES`] would otherwise allocate millions of them.
/// A hundred thousand lines is more file than anyone scrolls a preview
/// through; reading further is what the editor is for.
pub const MAX_LINES: usize = 100_000;

/// Wall-clock budget for syntax highlighting.
///
/// Highlighting runs on a background thread, so this is not about frames — it
/// is about the gap between pressing a key and seeing the file. A pathological
/// grammar against a megabyte of minified input can spend seconds per line;
/// when the budget runs out the rest of the document is kept as plain text,
/// which is strictly better than either waiting for it or cutting it off.
const HIGHLIGHT_BUDGET: std::time::Duration = std::time::Duration::from_millis(500);

/// A run of characters sharing one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    /// Foreground colour as RGB. Alpha from the theme is dropped: the previewer
    /// composites text over its own background, so a theme's translucency would
    /// read as washed-out rather than as intended.
    pub color: [u8; 3],
    pub bold: bool,
    pub italic: bool,
}

/// One line of a highlighted document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    /// The line as unstyled text, for measuring and for search.
    #[must_use]
    pub fn plain(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

/// A text file, highlighted and bounded.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub lines: Vec<Line>,
    /// Syntax name, e.g. "Rust". `None` when nothing matched and the file is
    /// shown as plain text.
    pub language: Option<String>,
    /// Set when the file was longer than the limits allow, so the frontend can
    /// say so instead of implying the file simply ends there.
    pub truncated: bool,
    /// Bytes actually read.
    pub bytes_read: usize,
    /// Whether the bytes had to be decoded lossily, which is the previewer's
    /// only signal that a "text" file is not actually UTF-8.
    pub lossy: bool,
    /// Whether this document is prose to be read rather than source to be
    /// inspected.
    ///
    /// The two want opposite typography — prose wraps in a proportional face
    /// with no line numbers, source is monospaced, numbered, and clipped — so
    /// the frontend needs to know which it was handed. Only the Markdown
    /// renderer sets it.
    pub prose: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// The syntax set, loaded once.
///
/// Loading is a few tens of milliseconds of binary deserialisation. Doing it per
/// preview would put that on the path between pressing space and seeing the
/// file, for every text file in a directory.
///
/// `two-face` rather than syntect's bundled defaults, which is the set
/// cosmic-edit uses. The defaults cover about forty languages and miss ones the
/// desktop entry claims to preview — TOML most visibly, along with TypeScript,
/// Dockerfiles, and most configuration formats. `extra_newlines` is the
/// newline-preserving variant, which is what [`LinesWithEndings`] below needs.
fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

/// COSMIC's dark syntax palette, as TextMate theme XML.
///
/// Checked in rather than pulled from `cosmic-syntax-theme`, which publishes
/// only as a git repository — and a crate with a git dependency cannot be
/// published to crates.io at all. See `themes/README.md` for provenance and
/// for how to regenerate these.
const COSMIC_DARK_TM_THEME: &str = include_str!("../themes/cosmic_dark.tmTheme");

/// COSMIC's light syntax palette. See [`COSMIC_DARK_TM_THEME`].
const COSMIC_LIGHT_TM_THEME: &str = include_str!("../themes/cosmic_light.tmTheme");

/// COSMIC's own syntax palettes, one per mode.
///
/// The desktop ships these, and cosmic-edit renders source with them, so a file
/// previewed here and the same file opened in the editor are colored alike.
/// Loading them replaces a compromise: syntect's bundled themes were designed
/// against their own backgrounds, and picking two that did not fight the panel
/// left a pair chosen for not clashing rather than for matching.
///
/// Backgrounds and gutters are zeroed for the same reason cosmic-edit zeroes
/// them — the panel behind the text is the previewer's own translucent surface,
/// and a theme that paints its own ground would cover it.
pub(crate) fn theme_for(dark: bool) -> Option<&'static Theme> {
    static THEMES: OnceLock<Option<(Theme, Theme)>> = OnceLock::new();

    THEMES
        .get_or_init(|| {
            let load = |data: &str| -> Option<Theme> {
                let mut theme = ThemeSet::load_from_reader(&mut std::io::Cursor::new(data)).ok()?;
                let transparent = syntect::highlighting::Color {
                    r: 0,
                    g: 0,
                    b: 0,
                    a: 0,
                };
                theme.settings.background = Some(transparent);
                theme.settings.gutter = Some(transparent);
                Some(theme)
            };

            match (load(COSMIC_DARK_TM_THEME), load(COSMIC_LIGHT_TM_THEME)) {
                (Some(dark), Some(light)) => Some((dark, light)),
                _ => {
                    // Compiled-in data, so this cannot happen without the
                    // dependency changing under us — but a preview must still
                    // open, unhighlighted, if it ever does.
                    tracing::warn!("COSMIC syntax themes failed to parse; showing plain text");
                    None
                }
            }
        })
        .as_ref()
        .map(|(dark_theme, light_theme)| if dark { dark_theme } else { light_theme })
}

/// Highlight a snippet of code by the name of its language.
///
/// For the fenced blocks inside a Markdown document, which are code without
/// being a file — there is no path to derive a syntax from, only the info
/// string the author wrote after the backticks.
///
/// Returns `None` when nothing in the syntax set answers to that name, which
/// is the caller's signal to show the block unhighlighted rather than to
/// guess at a language.
pub(crate) fn highlight_snippet(code: &str, language: &str, theme: &Theme) -> Option<Vec<Line>> {
    let syntaxes = syntaxes();
    // By name first ("Rust"), then by token, which is what an info string
    // actually carries ("rs", "rust").
    let syntax = syntaxes
        .find_syntax_by_name(language)
        .or_else(|| syntaxes.find_syntax_by_token(language))?;

    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut lines = Vec::new();

    for line in LinesWithEndings::from(code) {
        let Ok(styled) = highlighter.highlight_line(line, syntaxes) else {
            // Partial highlighting of a code block is worse than none: half
            // the block would be coloured and half not, which reads as a
            // rendering fault rather than as a limit.
            return None;
        };
        lines.push(Line {
            spans: styled
                .into_iter()
                .map(|(style, text)| Span {
                    text: text.trim_end_matches(['\n', '\r']).to_owned(),
                    color: [style.foreground.r, style.foreground.g, style.foreground.b],
                    bold: style.font_style.contains(FontStyle::BOLD),
                    italic: style.font_style.contains(FontStyle::ITALIC),
                })
                .filter(|span| !span.text.is_empty())
                .collect(),
        });
    }

    Some(lines)
}

/// Read and highlight a text file.
///
/// `dark` selects the palette; it comes from the desktop's theme, so a preview
/// never renders dark-on-dark after the user switches modes.
///
/// # Errors
///
/// Fails only when the file cannot be opened or read. A file that is not valid
/// UTF-8 is decoded lossily and flagged rather than rejected — a log with one
/// bad byte in it is still a log.
pub fn load(path: &Path, dark: bool) -> Result<Document, Error> {
    load_with_syntax(path, dark, None)
}

/// Read and highlight a text file, optionally naming its syntax.
///
/// `syntax` names the language rather than deriving it from the file name,
/// which is what a plugin rule declares and what a generated file — a command
/// plugin's output, written to a scratch path — needs, since its name says
/// nothing about its contents.
///
/// # Errors
///
/// As [`load`].
pub fn load_with_syntax(
    path: &Path,
    dark: bool,
    syntax_name: Option<&str>,
) -> Result<Document, Error> {
    let io = |source: std::io::Error| Error::Io {
        path: path.display().to_string(),
        source,
    };

    let file = std::fs::File::open(path).map_err(io)?;
    let mut buffer = Vec::new();
    // `take` bounds the read at the source rather than reading everything and
    // discarding the tail, which is the difference between touching 4 MB and
    // touching the whole file.
    let bytes_read = std::io::BufReader::new(file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(io)?;

    let over_bytes = bytes_read > MAX_BYTES;
    buffer.truncate(MAX_BYTES);

    let lossy = std::str::from_utf8(&buffer).is_err();
    let content = String::from_utf8_lossy(&buffer);
    // A byte-bounded read almost always cuts mid-line. Dropping the partial
    // tail is better than highlighting half a token as if it were whole.
    let content = if over_bytes {
        match content.rfind('\n') {
            Some(end) => &content[..=end],
            None => content.as_ref(),
        }
    } else {
        content.as_ref()
    };

    let syntaxes = syntaxes();
    // A named syntax is the caller's assertion and wins over the file name;
    // an unrecognised name falls through rather than losing the highlighting.
    let syntax = syntax_name
        .and_then(|name| {
            syntaxes
                .find_syntax_by_name(name)
                .or_else(|| syntaxes.find_syntax_by_token(name))
        })
        .or_else(|| syntaxes.find_syntax_for_file(path).ok().flatten())
        // Falling back on the *content* rather than on plain text catches
        // extensionless scripts, where the shebang is the only clue there is.
        .or_else(|| syntaxes.find_syntax_by_first_line(content.lines().next().unwrap_or("")));

    let Some(theme) = theme_for(dark) else {
        return Ok(plain(content, over_bytes, bytes_read, lossy));
    };

    let Some(syntax) = syntax else {
        return Ok(plain(content, over_bytes, bytes_read, lossy));
    };

    let started = std::time::Instant::now();
    let mut highlighter = Some(HighlightLines::new(syntax, theme));
    let mut lines = Vec::new();
    let mut over_lines = false;

    for line in LinesWithEndings::from(content) {
        if lines.len() >= MAX_LINES {
            over_lines = true;
            break;
        }

        // Checked every so many lines rather than every line: `elapsed` is a
        // clock read, and the budget is coarse enough that a batch is plenty.
        if highlighter.is_some() && lines.len() % 64 == 0 && started.elapsed() > HIGHLIGHT_BUDGET {
            tracing::debug!(
                lines = lines.len(),
                "highlight budget spent; continuing as plain text"
            );
            highlighter = None;
        }

        let styled = highlighter.as_mut().and_then(|highlighter| {
            match highlighter.highlight_line(line, syntaxes) {
                Ok(styled) => Some(styled),
                Err(error) => {
                    // A syntax definition can fail on pathological input. The
                    // rest of the file is still readable, so the highlighter is
                    // dropped and the document continues plain.
                    tracing::debug!(%error, "highlighting stopped early");
                    None
                }
            }
        });
        if styled.is_none() {
            highlighter = None;
        }

        lines.push(match styled {
            Some(styled) => Line {
                spans: styled
                    .into_iter()
                    .filter(|(_, text)| !text.is_empty())
                    .map(|(style, text)| Span {
                        text: text.trim_end_matches(['\n', '\r']).to_owned(),
                        color: [style.foreground.r, style.foreground.g, style.foreground.b],
                        bold: style.font_style.contains(FontStyle::BOLD),
                        italic: style.font_style.contains(FontStyle::ITALIC),
                    })
                    .filter(|span| !span.text.is_empty())
                    .collect(),
            },
            None => plain_line(line.trim_end_matches(['\n', '\r'])),
        });
    }

    Ok(Document {
        lines,
        language: Some(syntax.name.clone()),
        truncated: over_bytes || over_lines,
        bytes_read,
        lossy,
        prose: false,
    })
}

/// A single unhighlighted line.
pub(crate) fn plain_line(text: &str) -> Line {
    Line {
        spans: if text.is_empty() {
            Vec::new()
        } else {
            vec![Span {
                text: text.to_owned(),
                // Zeroed rather than a chosen grey — see [`plain`].
                color: [0, 0, 0],
                bold: false,
                italic: false,
            }]
        },
    }
}

/// Build an unhighlighted document, used when no syntax matches.
fn plain(content: &str, over_bytes: bool, bytes_read: usize, lossy: bool) -> Document {
    let mut over_lines = false;
    let mut lines = Vec::new();

    for line in content.lines() {
        if lines.len() >= MAX_LINES {
            over_lines = true;
            break;
        }
        // An all-zero colour means "use the theme's body text": plain documents
        // follow the desktop instead of a hard-coded shade that only works in
        // one mode.
        lines.push(plain_line(line));
    }

    Document {
        lines,
        language: None,
        truncated: over_bytes || over_lines,
        bytes_read,
        lossy,
        prose: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(name: &str, content: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("peek-test-{name}"));
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(content.as_bytes()).expect("write");
        path
    }

    #[test]
    fn rust_source_is_recognised_and_coloured() {
        let path = temp("highlight.rs", "fn main() {\n    let x = 1;\n}\n");
        let document = load(&path, true).expect("loads");

        assert_eq!(document.language.as_deref(), Some("Rust"));
        assert_eq!(document.lines.len(), 3);
        // Keywords and literals must not all come back the same colour, or the
        // highlighter silently did nothing.
        let colours: std::collections::HashSet<_> = document.lines[1]
            .spans
            .iter()
            .map(|span| span.color)
            .collect();
        assert!(colours.len() > 1, "expected more than one colour on a line");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn plain_text_still_produces_lines() {
        let path = temp("plain.unknownext", "alpha\nbeta\n");
        let document = load(&path, false).expect("loads");

        assert_eq!(document.lines.len(), 2);
        assert_eq!(document.lines[0].plain(), "alpha");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_utf8_is_flagged_not_rejected() {
        let path = std::env::temp_dir().join("peek-test-binary.txt");
        std::fs::write(&path, [0xff, 0xfe, b'h', b'i', b'\n']).expect("write");

        let document = load(&path, true).expect("loads");
        assert!(document.lossy);
        assert!(!document.lines.is_empty());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_file_is_an_error() {
        assert!(load(Path::new("/nonexistent/peek/file.txt"), true).is_err());
    }
}
