// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deciding which previewer a file gets.
//!
//! Detection is content-first, extension-second. The order matters: a previewer
//! is pointed at whatever the user selected, and file managers routinely show
//! `.txt` files that are really PNGs, `.bin` files that are really archives, and
//! extensionless files that are really source code. Trusting the name would mean
//! showing the wrong previewer for exactly the files where a preview is most
//! useful.
//!
//! The MIME database used is the system's `shared-mime-info`, which is also what
//! the file manager consults. Using a private table instead would let the
//! previewer disagree with the file manager about what a file *is* while both
//! are on screen.

use std::path::Path;

/// Which previewer renders a file.
///
/// Deliberately coarse. The distinctions that exist here are the ones that
/// change how the file is *drawn*; everything finer (JPEG vs PNG, tar vs zip) is
/// handled inside the relevant previewer, which already has to parse the file to
/// say anything about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// Decodable raster: PNG, JPEG, WebP, AVIF, TIFF, and friends.
    Image,
    /// Camera raw: CR2, NEF, ARW, DNG, and the rest. Separate from
    /// [`Kind::Image`] because the sensor data is not decoded — the previewer
    /// extracts the JPEG the camera embedded instead.
    CameraRaw,
    /// A comic book archive: images in a zip, read as pages.
    Comic,
    /// An EPUB: cover, metadata, and readable chapter text.
    Ebook,
    /// An office document whose text can be extracted: DOCX, ODT.
    Office,
    /// An installable font, shown as a specimen.
    Font,
    /// SVG. Separate from [`Kind::Image`] because it is rendered at the display
    /// size rather than decoded at a fixed one — rasterising an SVG to a
    /// thumbnail and then scaling it up is the one thing a vector previewer
    /// must not do.
    Vector,
    Pdf,
    /// Anything the user would expect to read: plain text, source, config,
    /// markup.
    Text,
    /// Markdown, rendered rather than shown as source.
    Markdown,
    Video,
    Audio,
    Archive,
    Directory,
    /// A third-party previewer claims this file. See [`crate::plugin`].
    Plugin,
    /// No previewer applies; the metadata card is shown instead.
    Other,
}

impl Kind {
    /// Whether this kind needs the whole surface rather than a reading column.
    ///
    /// Visual content is shown as large as it fits; text is not, because a line
    /// of prose 3440 px wide is unreadable however much room there is.
    #[must_use]
    pub fn is_visual(self) -> bool {
        matches!(
            self,
            Self::Image | Self::CameraRaw | Self::Comic | Self::Vector | Self::Pdf | Self::Video
        )
    }

    /// Whether the previewer has to keep a decoder running rather than
    /// producing one static result.
    #[must_use]
    pub fn is_timed(self) -> bool {
        matches!(self, Self::Video | Self::Audio)
    }
}

/// MIME types that are textual despite not living under `text/`.
///
/// `shared-mime-info` classifies most structured formats under `application/`,
/// so relying on the `text/` prefix alone would send JSON, XML, and every shell
/// script to the metadata card.
const TEXTUAL: &[&str] = &[
    "application/json",
    "application/ld+json",
    "application/xml",
    "application/xhtml+xml",
    "application/javascript",
    "application/ecmascript",
    "application/x-shellscript",
    "application/x-perl",
    "application/x-python",
    "application/x-ruby",
    "application/x-php",
    "application/x-yaml",
    "application/yaml",
    "application/toml",
    "application/sql",
    "application/x-desktop",
    "application/x-subrip",
    "application/pgp-keys",
    "application/pgp-signature",
    "application/x-x509-ca-cert",
    "image/x-xpixmap",
];

/// Archive and compression MIME types.
///
/// Listed explicitly rather than matched on a prefix: `application/` is where
/// almost everything unrecognised ends up, so a prefix rule would claim
/// unrelated formats and show them as empty archives.
const ARCHIVES: &[&str] = &[
    "application/zip",
    "application/x-zip-compressed",
    "application/vnd.rar",
    "application/x-rar-compressed",
    "application/x-tar",
    "application/x-compressed-tar",
    "application/x-bzip-compressed-tar",
    "application/x-bzip2-compressed-tar",
    "application/x-xz-compressed-tar",
    "application/x-lzma-compressed-tar",
    "application/zstd",
    "application/x-zstd-compressed-tar",
    "application/gzip",
    "application/x-bzip",
    "application/x-bzip2",
    "application/x-xz",
    "application/x-7z-compressed",
    "application/java-archive",
    "application/vnd.android.package-archive",
    "application/x-cd-image",
    "application/vnd.debian.binary-package",
    "application/x-rpm",
];

/// Extensions that mean Markdown.
///
/// `mdx` is included: its JSX extensions render as the literal text they are,
/// which is still a better preview than the punctuation.
const MARKDOWN_EXTENSIONS: &[&str] = &["md", "markdown", "mdown", "mkd", "mdx"];

/// Extensions whose content carries no distinguishing magic.
///
/// Content sniffing answers `text/plain` for every source file ever written,
/// which is *correct* and also useless for choosing a syntax. These are the
/// cases where the extension is the only evidence there is, so it is consulted
/// first — but only for files that sniffed as text, so a `.rs` file containing
/// a PNG is still shown as an image.
const TEXT_EXTENSIONS: &[&str] = &[
    "c",
    "cc",
    "cfg",
    "clj",
    "conf",
    "cpp",
    "cs",
    "css",
    "cxx",
    "d",
    "dart",
    "diff",
    "ex",
    "exs",
    "fish",
    "go",
    "gradle",
    "graphql",
    "h",
    "hpp",
    "hs",
    "htm",
    "html",
    "ini",
    "java",
    "jl",
    "js",
    "json",
    "jsonc",
    "jsx",
    "kt",
    "kts",
    "less",
    "lock",
    "lua",
    "md",
    "mdx",
    "ml",
    "nim",
    "nix",
    "patch",
    "php",
    "pl",
    "properties",
    "proto",
    "ps1",
    "py",
    "r",
    "rb",
    "rs",
    "sass",
    "scala",
    "scss",
    "sh",
    "sql",
    "svelte",
    "swift",
    "tf",
    "toml",
    "ts",
    "tsx",
    "txt",
    "vim",
    "vue",
    "xml",
    "yaml",
    "yml",
    "zig",
    "zsh",
];

/// Detect the MIME type and previewer for a path.
///
/// Returns the MIME type as well as the kind because the metadata card shows it
/// and the "open with" lookup needs it, and sniffing twice would mean reading
/// the file's magic twice.
#[must_use]
pub fn detect(path: &Path) -> (String, Kind) {
    if path.is_dir() {
        return ("inode/directory".to_owned(), Kind::Directory);
    }

    // `from_filepath` reads only the leading bytes, so this is one short read
    // rather than a scan of the file.
    let sniffed = tree_magic_mini::from_filepath(path).unwrap_or("application/octet-stream");
    let mime = refine(sniffed, path).to_owned();

    let kind = classify_with_plugins(&mime, path);
    (mime, kind)
}

/// Camera raw formats, as the `image/` subtypes `shared-mime-info` assigns.
///
/// The subtype rather than the whole MIME type, because the check sits inside
/// the `image/` branch and repeating the prefix sixteen times is noise. All of
/// these are TIFF containers the raw previewer can walk, except RAF and CR3 —
/// RAF has its own header the previewer also reads, and CR3 fails honestly
/// into a card until ISO BMFF parsing is worth adding.
const CAMERA_RAW: &[&str] = &[
    "x-adobe-dng",
    "x-canon-cr2",
    "x-canon-cr3",
    "x-canon-crw",
    "x-fuji-raf",
    "x-fujifilm-raf",
    "x-kodak-dcr",
    "x-kodak-kdc",
    "x-minolta-mrw",
    "x-nikon-nef",
    "x-nikon-nrw",
    "x-olympus-orf",
    "x-panasonic-raw",
    "x-panasonic-rw2",
    "x-pentax-pef",
    "x-samsung-srw",
    "x-sigma-x3f",
    "x-sony-arw",
    "x-sony-sr2",
    "x-sony-srf",
];

/// Types that describe a *container*, not a format.
///
/// A file that sniffs as one of these has been identified only as far as its
/// outermost layer: every SVG is XML, every shell script is plain text. The
/// magic database cannot go further, which is why `shared-mime-info` resolves
/// these cases from the file name — and why the file manager, which does
/// consult globs, will say `image/svg+xml` where bare magic says `text/xml`.
const GENERIC: &[&str] = &[
    "text/plain",
    "text/xml",
    "application/xml",
    "application/octet-stream",
];

/// Extensions that name a specific format inside a generic container.
///
/// Deliberately tiny. This is not a second MIME database — it lists only the
/// cases where the extension changes *which previewer runs*, because that is
/// the only thing worth overriding a content sniff for. A `.md` file sniffs as
/// plain text and previews as text either way, so it does not belong here;
/// an SVG sniffs as XML and would preview as source code, which is wrong.
const CONTAINED: &[(&str, &str)] = &[
    ("svg", "image/svg+xml"),
    ("svgz", "image/svg+xml-compressed"),
];

/// Extensions that name a specific format inside a *zip* container.
///
/// The same reasoning as [`CONTAINED`], one layer down: a comic, an ebook, and
/// a Word document are all zips to the magic database, and `shared-mime-info`
/// itself resolves them from the glob. Only listed where the previewer
/// changes — a `.jar` previews perfectly well as the archive it is.
const ZIP_CONTAINED: &[(&str, &str)] = &[
    ("cbz", "application/vnd.comicbook+zip"),
    ("epub", "application/epub+zip"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    ("odt", "application/vnd.oasis.opendocument.text"),
];

/// Narrow a generic sniff using the file name.
///
/// Only applies when the sniffed type is one of [`GENERIC`] or a bare zip, so
/// content still wins wherever content is decisive: a PNG named `diagram.svg`
/// keeps `image/png` and previews as the picture it is.
#[must_use]
fn refine<'a>(sniffed: &'a str, path: &Path) -> &'a str {
    let table: &[(&str, &str)] = if GENERIC.contains(&sniffed) {
        CONTAINED
    } else if sniffed == "application/zip" {
        ZIP_CONTAINED
    } else {
        return sniffed;
    };

    let Some(extension) = extension(path) else {
        return sniffed;
    };

    table
        .iter()
        .find(|(candidate, _)| *candidate == extension)
        .map_or(sniffed, |(_, mime)| mime)
}

/// Map a MIME type to a previewer.
///
/// Split out from [`detect`] so a caller that already knows the MIME type — the
/// D-Bus entry points are handed one by the file manager — does not pay for a
/// second sniff.
/// Map a MIME type to a previewer, consulting installed plugins.
///
/// Plugins are asked only where [`classify`] finds nothing, so a plugin can
/// add a preview and can never take one away — see [`crate::plugin`].
#[must_use]
pub fn classify_with_plugins(mime: &str, path: &Path) -> Kind {
    match classify(mime, path) {
        Kind::Other if crate::plugin::registry().find(mime, path).is_some() => Kind::Plugin,
        kind => kind,
    }
}

#[must_use]
pub fn classify(mime: &str, path: &Path) -> Kind {
    match mime {
        "inode/directory" => return Kind::Directory,
        "image/svg+xml" | "image/svg+xml-compressed" => return Kind::Vector,
        "application/pdf" | "application/x-pdf" => return Kind::Pdf,
        _ => {}
    }

    if ARCHIVES.contains(&mime) {
        return Kind::Archive;
    }

    if let Some(rest) = mime.strip_prefix("image/") {
        // Raw camera formats sniff as `image/x-*` but the `image` crate cannot
        // decode them; they get the embedded-preview extractor instead of the
        // raster decoder.
        if CAMERA_RAW.contains(&rest) {
            return Kind::CameraRaw;
        }
        return Kind::Image;
    }

    if mime.starts_with("video/") {
        return Kind::Video;
    }
    if mime.starts_with("audio/") {
        return Kind::Audio;
    }

    match mime {
        "application/vnd.comicbook+zip" | "application/x-cbz" => return Kind::Comic,
        "application/epub+zip" => return Kind::Ebook,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "application/vnd.oasis.opendocument.text" => return Kind::Office,
        // The formats ttf-parser reads. WOFF and WOFF2 are compressed
        // wrappers it does not unpack, so they stay on the metadata card
        // rather than promising a specimen that would fail.
        "font/ttf"
        | "font/otf"
        | "font/collection"
        | "application/x-font-ttf"
        | "application/x-font-otf"
        | "application/vnd.ms-opentype" => return Kind::Font,
        _ => {}
    }

    if mime.starts_with("text/") || TEXTUAL.contains(&mime) {
        // Markdown is text that is *read*, not text that is inspected, so it
        // is rendered. Decided on the extension because Markdown has no magic
        // of its own — but only for files that already sniffed as text, so a
        // PNG named `.md` is still a PNG.
        if mime == "text/markdown"
            || extension(path).is_some_and(|ext| MARKDOWN_EXTENSIONS.contains(&ext.as_str()))
        {
            return Kind::Markdown;
        }
        return Kind::Text;
    }

    // Extensionless scripts and unusual source languages sniff as
    // `application/octet-stream`; the extension is the remaining evidence.
    if extension(path).is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext.as_str())) {
        return Kind::Text;
    }

    Kind::Other
}

/// Lower-cased extension, if there is one.
#[must_use]
pub fn extension(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn source_files_are_text() {
        // Sniffed content wins, but a Rust file has no magic of its own, so the
        // extension is what carries it.
        assert_eq!(
            classify("text/plain", &PathBuf::from("main.rs")),
            Kind::Text
        );
        assert_eq!(
            classify("application/octet-stream", &PathBuf::from("build.zig")),
            Kind::Text
        );
    }

    #[test]
    fn svg_is_not_a_raster() {
        assert_eq!(
            classify("image/svg+xml", &PathBuf::from("logo.svg")),
            Kind::Vector
        );
    }

    #[test]
    fn structured_text_is_text() {
        assert_eq!(
            classify("application/json", &PathBuf::from("package.json")),
            Kind::Text
        );
    }

    #[test]
    fn unknown_binaries_fall_through_to_metadata() {
        assert_eq!(
            classify("application/octet-stream", &PathBuf::from("core.dump")),
            Kind::Other
        );
    }

    #[test]
    fn content_beats_the_name() {
        // The whole reason detection is content-first: a mislabelled file must
        // still get the previewer its bytes deserve.
        assert_eq!(
            classify("image/png", &PathBuf::from("notes.txt")),
            Kind::Image
        );
    }

    #[test]
    fn an_svg_is_narrowed_from_the_xml_it_sniffs_as() {
        // Bare magic cannot tell an SVG from any other XML document, so without
        // this an SVG previews as source code rather than as a drawing.
        assert_eq!(
            refine("text/xml", &PathBuf::from("logo.svg")),
            "image/svg+xml"
        );
        assert_eq!(
            refine("text/plain", &PathBuf::from("logo.svg")),
            "image/svg+xml"
        );
    }

    #[test]
    fn narrowing_never_overrides_a_decisive_sniff() {
        // A PNG that someone named `.svg` is still a PNG.
        assert_eq!(refine("image/png", &PathBuf::from("logo.svg")), "image/png");
    }

    #[test]
    fn narrowing_leaves_unlisted_extensions_alone() {
        assert_eq!(
            refine("text/plain", &PathBuf::from("notes.md")),
            "text/plain"
        );
        assert_eq!(
            refine("text/plain", &PathBuf::from("no-extension")),
            "text/plain"
        );
    }

    #[test]
    fn markdown_is_rendered_rather_than_shown_as_source() {
        assert_eq!(
            classify("text/plain", &PathBuf::from("README.md")),
            Kind::Markdown
        );
        assert_eq!(
            classify("text/markdown", &PathBuf::from("notes")),
            Kind::Markdown
        );
        // Content still wins: a PNG named `.md` is a PNG.
        assert_eq!(
            classify("image/png", &PathBuf::from("README.md")),
            Kind::Image
        );
        // And ordinary source is untouched.
        assert_eq!(
            classify("text/plain", &PathBuf::from("main.rs")),
            Kind::Text
        );
    }

    #[test]
    fn zip_containers_are_narrowed_by_extension() {
        // Magic alone says "zip" for all of these; the glob is what
        // shared-mime-info itself uses to tell them apart.
        assert_eq!(
            refine("application/zip", &PathBuf::from("issue-01.cbz")),
            "application/vnd.comicbook+zip"
        );
        assert_eq!(
            refine("application/zip", &PathBuf::from("novel.epub")),
            "application/epub+zip"
        );
        assert_eq!(
            refine("application/zip", &PathBuf::from("report.docx")),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        // A zip that is just a zip stays one.
        assert_eq!(
            refine("application/zip", &PathBuf::from("backup.zip")),
            "application/zip"
        );
    }

    #[test]
    fn documents_comics_ebooks_and_fonts_get_their_previewers() {
        assert_eq!(
            classify("application/vnd.comicbook+zip", &PathBuf::from("a.cbz")),
            Kind::Comic
        );
        assert_eq!(
            classify("application/epub+zip", &PathBuf::from("a.epub")),
            Kind::Ebook
        );
        assert_eq!(
            classify(
                "application/vnd.oasis.opendocument.text",
                &PathBuf::from("a.odt")
            ),
            Kind::Office
        );
        assert_eq!(classify("font/ttf", &PathBuf::from("a.ttf")), Kind::Font);
        // Compressed font wrappers are not promised a specimen.
        assert_eq!(
            classify("font/woff2", &PathBuf::from("a.woff2")),
            Kind::Other
        );
    }

    #[test]
    fn raw_photos_get_the_embedded_preview_extractor() {
        assert_eq!(
            classify("image/x-canon-cr2", &PathBuf::from("IMG_0001.CR2")),
            Kind::CameraRaw
        );
        assert_eq!(
            classify("image/x-adobe-dng", &PathBuf::from("scan.dng")),
            Kind::CameraRaw
        );
        // An unlisted image subtype still goes to the ordinary decoder.
        assert_eq!(
            classify("image/x-tga", &PathBuf::from("texture.tga")),
            Kind::Image
        );
    }
}
