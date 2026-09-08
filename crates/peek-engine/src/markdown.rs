// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Markdown, rendered rather than shown as source.
//!
//! A `.md` file previews as source perfectly well, and that is what it used to
//! do — but nobody writes Markdown to read the asterisks. Rendering it is the
//! difference between previewing a README and previewing its punctuation.
//!
//! ## No HTML anywhere near this
//!
//! The obvious way to render Markdown is to convert it to HTML and show that
//! in a webview, and every previewer that did so came to regret it: a browser
//! engine is tens of megabytes, a second rendering stack, and a remote-content
//! attack surface pointed at a file the user merely hovered over. Instead
//! `pulldown-cmark`'s event stream is turned straight into the same
//! [`Document`] of coloured spans that source files produce, so the frontend
//! draws Markdown with the widget it already has and inline HTML is shown as
//! the literal text it is.
//!
//! ## Colours come from the desktop
//!
//! Headings, links, and code are styled by asking the *same* COSMIC syntax
//! theme that colours source files for its `markup.*` scopes — the scopes
//! every TextMate-derived theme defines for exactly this. A README previewed
//! here and one opened in cosmic-edit are therefore coloured by one authority
//! rather than by a palette invented here.

use std::path::Path;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use syntect::highlighting::{FontStyle, Highlighter, Theme};
use syntect::parsing::Scope;

use crate::text::{Document, Line, Span};

/// Bytes of Markdown read.
///
/// The same ceiling source files get: far more than fits on a screen, and
/// small enough that parsing it is never the reason a preview is late.
const MAX_BYTES: usize = crate::text::MAX_BYTES;

/// Rendered lines kept.
///
/// Lower than the ceiling source files get, because prose is drawn wrapped
/// and therefore *unwindowed* — every line is shaped. Twenty thousand lines
/// is a book's worth of README and still bounded work.
const MAX_LINES: usize = 20_000;

/// How deep a nested list is indented, in spaces per level.
const INDENT: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Render a Markdown file into styled lines.
///
/// # Errors
///
/// Fails only when the file cannot be read. Malformed Markdown does not
/// exist — CommonMark parses anything — so there is no second failure mode.
pub fn render(path: &Path, dark: bool) -> Result<Document, Error> {
    use std::io::Read;

    let file = std::fs::File::open(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;

    let mut buffer = Vec::new();
    let bytes_read = std::io::BufReader::new(file)
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut buffer)
        .map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?;

    let over_bytes = bytes_read > MAX_BYTES;
    buffer.truncate(MAX_BYTES);

    let lossy = std::str::from_utf8(&buffer).is_err();
    let source = String::from_utf8_lossy(&buffer);

    let mut document = render_str(&source, dark);
    document.truncated |= over_bytes;
    document.bytes_read = bytes_read;
    document.lossy = lossy;
    Ok(document)
}

/// Render Markdown already in memory.
///
/// Split out from [`render`] so the rendering itself is testable without a
/// file, and so a caller that already holds the text — a plugin's output —
/// does not have to write it to disk first.
#[must_use]
pub fn render_str(source: &str, dark: bool) -> Document {
    let palette = Palette::new(dark);

    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);

    let mut writer = Writer::new(&palette);
    for event in Parser::new_ext(source, options) {
        if writer.lines.len() >= MAX_LINES {
            writer.truncated = true;
            break;
        }
        writer.handle(event);
    }
    writer.finish()
}

/// The colours and weights the renderer draws with.
///
/// Resolved once per render from the desktop's syntax theme, so a document is
/// styled by the same authority that colours source — and so the lookup, which
/// walks the theme's scope selectors, happens once rather than per span.
struct Palette {
    heading: Style,
    link: Style,
    code: Style,
    quote: Style,
    /// Body text. Zeroed, which the frontend reads as "use the theme's own
    /// text colour" — see [`crate::text`].
    body: Style,
    theme: Option<&'static Theme>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Style {
    color: [u8; 3],
    bold: bool,
    italic: bool,
}

impl Style {
    const BODY: Self = Self {
        color: [0, 0, 0],
        bold: false,
        italic: false,
    };

    fn bold(self) -> Self {
        Self { bold: true, ..self }
    }

    fn italic(self) -> Self {
        Self {
            italic: true,
            ..self
        }
    }
}

impl Palette {
    fn new(dark: bool) -> Self {
        let theme = crate::text::theme_for(dark);
        let styled = |selector: &str, fallback: Style| -> Style {
            let Some(theme) = theme else {
                return fallback;
            };
            let Ok(scope) = Scope::new(selector) else {
                return fallback;
            };
            let resolved = Highlighter::new(theme).style_for_stack(&[scope]);

            // A theme that has no opinion about a scope resolves it to the
            // same foreground it gives everything else; that is not a colour
            // worth carrying, so it falls back to the body convention.
            let colour = [
                resolved.foreground.r,
                resolved.foreground.g,
                resolved.foreground.b,
            ];
            let default = theme.settings.foreground.map(|fg| [fg.r, fg.g, fg.b]);
            if Some(colour) == default {
                return fallback;
            }

            Style {
                color: colour,
                bold: resolved.font_style.contains(FontStyle::BOLD) || fallback.bold,
                italic: resolved.font_style.contains(FontStyle::ITALIC) || fallback.italic,
            }
        };

        Self {
            heading: styled("markup.heading", Style::BODY.bold()),
            link: styled("markup.underline.link", Style::BODY),
            code: styled("markup.raw.inline", Style::BODY),
            quote: styled("markup.quote", Style::BODY.italic()),
            body: Style::BODY,
            theme,
        }
    }
}

/// Turns the parser's event stream into lines.
struct Writer<'a> {
    palette: &'a Palette,
    lines: Vec<Line>,
    /// Spans accumulated for the line currently being built.
    current: Vec<Span>,
    /// Inline style stack: emphasis, strong, links and code nest.
    styles: Vec<Style>,
    /// Open list markers, innermost last. `Some(n)` counts an ordered list.
    lists: Vec<Option<u64>>,
    /// Depth of block quoting, drawn as a leading rule per line.
    quote_depth: usize,
    /// The language of the fenced block being read, when inside one.
    code_language: Option<String>,
    /// Raw code accumulated inside a fence, highlighted when it closes.
    code_buffer: String,
    in_code_block: bool,
    truncated: bool,
}

impl<'a> Writer<'a> {
    fn new(palette: &'a Palette) -> Self {
        Self {
            palette,
            lines: Vec::new(),
            current: Vec::new(),
            styles: Vec::new(),
            lists: Vec::new(),
            quote_depth: 0,
            code_language: None,
            code_buffer: String::new(),
            in_code_block: false,
            truncated: false,
        }
    }

    /// The style inline text is currently written in.
    fn style(&self) -> Style {
        *self.styles.last().unwrap_or(&self.palette.body)
    }

    fn push_text(&mut self, text: &str, style: Style) {
        if text.is_empty() {
            return;
        }
        self.current.push(Span {
            text: text.to_owned(),
            color: style.color,
            bold: style.bold,
            italic: style.italic,
        });
    }

    /// End the line being built, keeping blank lines that separate blocks.
    fn break_line(&mut self) {
        if self.lines.len() >= MAX_LINES {
            self.truncated = true;
            return;
        }
        let spans = std::mem::take(&mut self.current);
        self.lines.push(Line { spans });
    }

    /// End a block, leaving one blank line after it.
    ///
    /// Collapsed rather than counted: two blank lines between paragraphs is
    /// the source's formatting, not the document's, and a preview that
    /// reproduces them wastes the panel.
    fn end_block(&mut self) {
        if !self.current.is_empty() {
            self.break_line();
        }
        if self.lines.last().is_some_and(|line| !line.spans.is_empty()) {
            self.break_line();
        }
    }

    /// The indent and quote rule every line inside a block starts with.
    fn open_line(&mut self) {
        for _ in 0..self.quote_depth {
            // A rule rather than the source's ">": the character is Markdown's
            // syntax, and the point of rendering is to show what it meant.
            self.push_text("\u{2502} ", self.palette.quote);
        }
        let indent = self.lists.len().saturating_sub(1) * INDENT;
        if indent > 0 {
            self.push_text(&" ".repeat(indent), self.palette.body);
        }
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),

            Event::Text(text) => {
                if self.in_code_block {
                    self.code_buffer.push_str(&text);
                    return;
                }
                // A block's own text can span source lines; those breaks are
                // the author's wrapping, not paragraph breaks.
                let style = self.style();
                for (index, part) in text.split('\n').enumerate() {
                    if index > 0 {
                        self.break_line();
                        self.open_line();
                    }
                    self.push_text(part, style);
                }
            }

            Event::Code(code) => {
                let style = self.palette.code;
                self.push_text(&format!("\u{a0}{code}\u{a0}"), style);
            }

            // Inline and block HTML are shown as the text they are. Rendering
            // them would mean being a browser; hiding them would mean lying
            // about the file's contents.
            Event::Html(html) | Event::InlineHtml(html) => {
                let style = self.palette.code;
                for (index, part) in html.trim_end().split('\n').enumerate() {
                    if index > 0 {
                        self.break_line();
                        self.open_line();
                    }
                    self.push_text(part, style);
                }
            }

            Event::SoftBreak => {
                let style = self.style();
                self.push_text(" ", style);
            }
            Event::HardBreak => {
                self.break_line();
                self.open_line();
            }

            Event::Rule => {
                self.end_block();
                let style = self.palette.quote;
                self.push_text(&"\u{2500}".repeat(48), style);
                self.end_block();
            }

            Event::TaskListMarker(done) => {
                let style = self.palette.body;
                self.push_text(if done { "[x] " } else { "[ ] " }, style);
            }

            Event::FootnoteReference(label) => {
                let style = self.palette.link;
                self.push_text(&format!("[{label}]"), style);
            }

            // Maths is shown as its source: rendering TeX is a typesetting
            // engine's job, and the source is what the author wrote.
            Event::InlineMath(math) | Event::DisplayMath(math) => {
                let style = self.palette.code;
                self.push_text(&format!("\u{a0}{math}\u{a0}"), style);
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.end_block();
                self.open_line();
            }

            Tag::Heading { level, .. } => {
                self.end_block();
                self.open_line();
                // The hashes are kept: they are how a reader tells an H2 from
                // an H3 when every heading is drawn at one text size.
                let hashes = "#".repeat(heading_depth(level));
                let style = self.palette.heading;
                self.push_text(&format!("{hashes} "), style);
                self.styles.push(style);
            }

            Tag::BlockQuote(_) => {
                self.end_block();
                self.quote_depth += 1;
            }

            Tag::CodeBlock(kind) => {
                self.end_block();
                self.in_code_block = true;
                self.code_language = match kind {
                    CodeBlockKind::Fenced(language) if !language.is_empty() => {
                        // Info strings carry more than a language: ```rust,ignore
                        Some(
                            language
                                .split([',', ' '])
                                .next()
                                .unwrap_or_default()
                                .to_owned(),
                        )
                    }
                    _ => None,
                };
            }

            Tag::List(first) => {
                if self.lists.is_empty() {
                    self.end_block();
                }
                self.lists.push(first);
            }

            Tag::Item => {
                if !self.current.is_empty() {
                    self.break_line();
                }
                self.open_line();

                let marker = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let marker = format!("{number}. ");
                        *number += 1;
                        marker
                    }
                    _ => "\u{2022} ".to_owned(),
                };
                let style = self.palette.body;
                self.push_text(&marker, style);
            }

            Tag::Emphasis => self.styles.push(self.style().italic()),
            Tag::Strong => self.styles.push(self.style().bold()),
            Tag::Strikethrough | Tag::Superscript | Tag::Subscript => {
                self.styles.push(self.style());
            }

            Tag::Link { .. } => self.styles.push(self.palette.link),
            Tag::Image { dest_url, .. } => {
                // The alt text follows as ordinary text events, so this only
                // marks that an image was here — the panel is a preview of the
                // *document*, and fetching its images is not previewing.
                let style = self.palette.link;
                self.push_text(&format!("[image: {dest_url}] "), style);
                self.styles.push(style);
            }

            // Tables are laid out as tab-separated rows. A real column layout
            // needs measured text, which the engine deliberately cannot do —
            // it does not know the font the frontend will draw with.
            Tag::Table(_) | Tag::TableHead | Tag::TableRow => {
                self.end_block();
                self.open_line();
            }
            Tag::TableCell => {
                if !self.current.is_empty() {
                    let style = self.palette.body;
                    self.push_text("\t", style);
                }
            }

            Tag::FootnoteDefinition(label) => {
                self.end_block();
                self.open_line();
                let style = self.palette.link;
                self.push_text(&format!("[{label}]: "), style);
            }

            Tag::HtmlBlock | Tag::MetadataBlock(_) => self.end_block(),
            Tag::DefinitionList => self.end_block(),
            Tag::DefinitionListTitle => {
                self.end_block();
                self.open_line();
                self.styles.push(self.palette.body.bold());
            }
            Tag::DefinitionListDefinition => {
                self.break_line();
                self.open_line();
                let style = self.palette.body;
                self.push_text(&" ".repeat(INDENT), style);
            }
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.end_block(),

            TagEnd::Heading(_) => {
                self.styles.pop();
                self.end_block();
            }

            TagEnd::BlockQuote(_) => {
                self.quote_depth = self.quote_depth.saturating_sub(1);
                self.end_block();
            }

            TagEnd::CodeBlock => {
                self.in_code_block = false;
                let code = std::mem::take(&mut self.code_buffer);
                let language = self.code_language.take();
                self.push_code(&code, language.as_deref());
                self.end_block();
            }

            TagEnd::List(_) => {
                self.lists.pop();
                if self.lists.is_empty() {
                    self.end_block();
                }
            }
            TagEnd::Item => {
                if !self.current.is_empty() {
                    self.break_line();
                }
            }

            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image
            | TagEnd::DefinitionListTitle => {
                self.styles.pop();
            }

            TagEnd::TableHead | TagEnd::TableRow => self.break_line(),
            TagEnd::Table => self.end_block(),
            TagEnd::TableCell => {}

            TagEnd::FootnoteDefinition
            | TagEnd::HtmlBlock
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListDefinition => self.end_block(),
        }
    }

    /// Emit a fenced block, syntax-highlighted where the language is known.
    ///
    /// The same highlighter source files go through, so Rust in a README and
    /// Rust in a `.rs` file are coloured identically.
    fn push_code(&mut self, code: &str, language: Option<&str>) {
        let highlighted = self
            .palette
            .theme
            .zip(language)
            .and_then(|(theme, language)| crate::text::highlight_snippet(code, language, theme));

        match highlighted {
            Some(lines) => {
                for line in lines {
                    if self.lines.len() >= MAX_LINES {
                        self.truncated = true;
                        return;
                    }
                    self.open_line();
                    self.push_text("    ", self.palette.body);
                    self.current.extend(line.spans);
                    self.break_line();
                }
            }
            None => {
                let style = self.palette.code;
                for line in code.lines() {
                    if self.lines.len() >= MAX_LINES {
                        self.truncated = true;
                        return;
                    }
                    self.open_line();
                    self.push_text(&format!("    {line}"), style);
                    self.break_line();
                }
            }
        }
    }

    fn finish(mut self) -> Document {
        if !self.current.is_empty() {
            self.break_line();
        }
        // A document that ends in blank lines wastes the panel's height, and
        // the blanks are an artefact of block separation rather than content.
        while self.lines.last().is_some_and(|line| line.spans.is_empty()) {
            self.lines.pop();
        }

        Document {
            lines: self.lines,
            language: Some("Markdown".to_owned()),
            truncated: self.truncated,
            bytes_read: 0,
            lossy: false,
            prose: true,
        }
    }
}

/// Heading level as a number of hashes.
fn heading_depth(level: HeadingLevel) -> usize {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(document: &Document) -> Vec<String> {
        document.lines.iter().map(Line::plain).collect()
    }

    #[test]
    fn headings_keep_their_level_and_are_styled() {
        let document = render_str("# Title\n\n## Section\n", true);
        let text = plain(&document);

        assert!(text.contains(&"# Title".to_owned()), "got {text:?}");
        assert!(text.contains(&"## Section".to_owned()));

        // A heading must not render as body text, or the structure is lost.
        let heading = document
            .lines
            .iter()
            .find(|line| line.plain().starts_with("# Title"))
            .expect("the heading");
        assert!(
            heading.spans.iter().any(|span| span.bold),
            "a heading should be emphasised"
        );
    }

    #[test]
    fn emphasis_becomes_style_rather_than_punctuation() {
        let document = render_str("Some *emphasis* and **strength**.", true);
        let text = plain(&document);

        assert_eq!(
            text,
            vec!["Some emphasis and strength."],
            "markers are gone"
        );
        assert!(document.lines[0].spans.iter().any(|span| span.italic));
        assert!(document.lines[0].spans.iter().any(|span| span.bold));
    }

    #[test]
    fn lists_get_markers_and_numbering() {
        let document = render_str("- one\n- two\n\n1. first\n2. second\n", true);
        let text = plain(&document);

        assert!(text.contains(&"\u{2022} one".to_owned()), "got {text:?}");
        assert!(text.contains(&"\u{2022} two".to_owned()));
        assert!(text.contains(&"1. first".to_owned()));
        assert!(text.contains(&"2. second".to_owned()));
    }

    #[test]
    fn fenced_code_is_highlighted_by_its_language() {
        let document = render_str("```rust\nfn main() {}\n```\n", true);
        let code = document
            .lines
            .iter()
            .find(|line| line.plain().contains("fn main"))
            .expect("the code line");

        let colours: std::collections::HashSet<_> =
            code.spans.iter().map(|span| span.color).collect();
        assert!(
            colours.len() > 1,
            "a known language should be highlighted, got {colours:?}"
        );
    }

    #[test]
    fn an_unknown_language_still_renders_as_code() {
        let document = render_str("```notalanguage\nsome text\n```\n", true);
        assert!(
            plain(&document)
                .iter()
                .any(|line| line.contains("some text")),
            "the fence's contents must still be shown"
        );
    }

    #[test]
    fn html_is_shown_as_text_rather_than_rendered_or_hidden() {
        let document = render_str("<div class=\"x\">hi</div>\n", true);
        let text = plain(&document).join("\n");
        assert!(text.contains("<div"), "inline HTML is shown literally");
    }

    #[test]
    fn blockquotes_are_drawn_with_a_rule_not_a_chevron() {
        let document = render_str("> quoted\n", true);
        let text = plain(&document).join("\n");
        assert!(text.contains('\u{2502}'), "got {text:?}");
        assert!(!text.contains('>'), "the source marker should be gone");
    }

    #[test]
    fn links_keep_their_text_and_are_styled() {
        let document = render_str("See [the docs](https://example.com).", true);
        assert_eq!(plain(&document), vec!["See the docs."]);
    }

    #[test]
    fn a_document_does_not_end_in_blank_lines() {
        let document = render_str("Text.\n\n\n\n", true);
        assert_eq!(plain(&document), vec!["Text."]);
    }

    #[test]
    fn tables_render_their_cells() {
        let document = render_str("| a | b |\n| - | - |\n| 1 | 2 |\n", true);
        let text = plain(&document).join("\n");
        assert!(text.contains('a') && text.contains('2'), "got {text:?}");
    }

    #[test]
    fn a_missing_file_is_an_error() {
        assert!(render(Path::new("/nonexistent/peek/readme.md"), true).is_err());
    }
}
