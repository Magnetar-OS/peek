// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! End-to-end previewer tests against real files.
//!
//! The unit tests in each module cover the decisions — which previewer a MIME
//! type gets, how a duration formats, where the panel goes. What they cannot
//! cover is whether the previewers actually decode anything, because that
//! depends on `image`, `resvg`, `poppler`, and `zip` behaving as expected
//! against bytes on disk.
//!
//! So these build genuine files and run the real dispatcher over them. Every
//! fixture is generated rather than committed: a checked-in PNG proves the
//! decoder can read *that* PNG, and it silently stops proving anything the day
//! the encoder changes.

use std::io::Write;
use std::path::{Path, PathBuf};

use peek_engine::{Entry, Options, Preview, meta, preview};

/// A directory that removes itself, so a failing test does not leave fixtures
/// behind for the next run to trip over.
struct Fixtures(PathBuf);

impl Fixtures {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("peek-fixtures-{name}"));
        // Removed first rather than trusted to be absent: a previous run that
        // panicked mid-test would otherwise leave files that change what these
        // assertions see.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create the fixture directory");
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, bytes).expect("write a fixture");
        path
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn load(path: &Path) -> Preview {
    let entry = Entry::load(path).expect("the fixture can be stat'd");
    preview::load(&entry, Options::default())
}

#[test]
fn a_png_decodes_to_its_own_dimensions() {
    let fixtures = Fixtures::new("png");

    let mut image = image::RgbaImage::new(64, 32);
    // A gradient rather than a flat fill: a decoder that returns the right
    // dimensions but the wrong pixels would pass against a solid colour.
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        *pixel = image::Rgba([(x * 4) as u8, (y * 8) as u8, 128, 255]);
    }

    let path = fixtures.path("gradient.png");
    image.save(&path).expect("encode the PNG");

    match load(&path) {
        Preview::Picture(picture) => {
            assert_eq!((picture.source_width, picture.source_height), (64, 32));
            assert_eq!(picture.raster.width, 64);
            assert_eq!(picture.raster.height, 32);
            assert_eq!(picture.raster.pixels.len(), 64 * 32 * 4);

            // The gradient must survive the round trip, not just the geometry.
            let top_left = &picture.raster.pixels[..4];
            assert_eq!(top_left, [0, 0, 128, 255]);
        }
        other => panic!("expected a picture, got {other:?}"),
    }
}

#[test]
fn an_oversized_image_is_reduced_but_keeps_its_reported_size() {
    let fixtures = Fixtures::new("large");

    // Wider than `picture::MAX_EDGE`, so the reduction path runs.
    let image = image::RgbaImage::new(5000, 1000);
    let path = fixtures.path("wide.png");
    image.save(&path).expect("encode the PNG");

    match load(&path) {
        Preview::Picture(picture) => {
            assert_eq!(
                picture.source_width, 5000,
                "the file's own size is reported"
            );
            assert!(
                picture.raster.width <= peek_engine::picture::MAX_EDGE,
                "the decoded raster is bounded, got {}",
                picture.raster.width
            );
            // Aspect ratio has to survive the reduction, or a wide image
            // previews stretched.
            let aspect = picture.raster.aspect();
            assert!((aspect - 5.0).abs() < 0.05, "aspect drifted to {aspect}");
        }
        other => panic!("expected a picture, got {other:?}"),
    }
}

#[test]
fn an_svg_renders_at_the_requested_size_rather_than_its_own() {
    let fixtures = Fixtures::new("svg");

    let path = fixtures.write(
        "square.svg",
        // Two hashes: a single-hash raw string would terminate at the `"#` in
        // the colour literal.
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
             <rect width="10" height="10" fill="#3584e4"/>
           </svg>"##,
    );

    let entry = Entry::load(&path).expect("stats");
    assert_eq!(entry.kind, peek_engine::Kind::Vector);

    let rendered = preview::load(
        &entry,
        Options {
            vector_target: 800,
            ..Options::default()
        },
    );

    match rendered {
        Preview::Picture(picture) => {
            // The whole point of a vector previewer: a 10px document must come
            // back at 800px, not at 10px to be scaled up later.
            assert_eq!(picture.raster.width, 800);
            assert_eq!(picture.raster.height, 800);
            assert_eq!(
                (picture.source_width, picture.source_height),
                (10, 10),
                "the document's own size is still reported"
            );

            // The fill must actually be painted, not left transparent.
            let centre = ((400 * 800 + 400) * 4) as usize;
            assert_eq!(picture.raster.pixels[centre + 3], 255, "expected opaque");
        }
        other => panic!("expected a rendered vector, got {other:?}"),
    }
}

#[test]
fn source_is_highlighted_and_named() {
    let fixtures = Fixtures::new("source");
    let path = fixtures.write(
        "lib.rs",
        b"//! A doc comment.\npub fn answer() -> u32 {\n    42\n}\n",
    );

    match load(&path) {
        Preview::Text(document) => {
            assert_eq!(document.language.as_deref(), Some("Rust"));
            assert_eq!(document.lines.len(), 4);
            assert!(!document.truncated);
            assert!(!document.lossy);
            assert_eq!(document.lines[2].plain().trim(), "42");
        }
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn a_long_file_is_truncated_and_says_so() {
    let fixtures = Fixtures::new("long");

    let mut content = String::new();
    for line in 0..(peek_engine::text::MAX_LINES + 500) {
        content.push_str(&format!("line {line}\n"));
    }
    let path = fixtures.write("long.log", content.as_bytes());

    match load(&path) {
        Preview::Text(document) => {
            assert!(document.truncated, "a longer file must report truncation");
            assert_eq!(document.lines.len(), peek_engine::text::MAX_LINES);
        }
        other => panic!("expected text, got {other:?}"),
    }
}

#[test]
fn a_zip_lists_its_members_without_extracting_them() {
    let fixtures = Fixtures::new("zip");
    let path = fixtures.path("bundle.zip");

    {
        let file = std::fs::File::create(&path).expect("create the zip");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        writer
            .add_directory("docs/", options)
            .expect("add a directory");
        writer
            .start_file("docs/readme.txt", options)
            .expect("start");
        writer
            .write_all(b"hello from inside the archive")
            .expect("write");
        writer.start_file("data.bin", options).expect("start");
        writer.write_all(&[7u8; 1024]).expect("write");
        writer.finish().expect("finish the zip");
    }

    let before: Vec<_> = std::fs::read_dir(&fixtures.0)
        .expect("list")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect();

    match load(&path) {
        Preview::Archive(archive) => {
            assert_eq!(archive.format, "Zip");
            assert_eq!(archive.members.len(), 3);
            assert!(archive.members.iter().any(|member| member.is_dir));
            assert!(
                archive
                    .members
                    .iter()
                    .any(|member| member.name == "data.bin" && member.size == 1024)
            );
            assert_eq!(
                archive.total_size,
                1024 + "hello from inside the archive".len() as u64
            );
            assert!(!archive.truncated);
        }
        other => panic!("expected an archive, got {other:?}"),
    }

    let after: Vec<_> = std::fs::read_dir(&fixtures.0)
        .expect("list")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(before, after, "listing an archive must not write anything");
}

#[test]
fn a_compressed_tar_lists_through_its_decompressor() {
    use std::io::Read;

    let fixtures = Fixtures::new("tar");
    let path = fixtures.path("bundle.tar.gz");

    {
        let file = std::fs::File::create(&path).expect("create");
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        let mut builder = tar::Builder::new(encoder);

        let payload = b"tar member contents";
        let mut header = tar::Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "notes.txt", &payload[..])
            .expect("append");
        builder.into_inner().expect("flush").finish().expect("gzip");
    }

    // Confirm the fixture really is gzip, so a failure below is the previewer's
    // and not the fixture's.
    let mut magic = [0u8; 2];
    std::fs::File::open(&path)
        .expect("open")
        .read_exact(&mut magic)
        .expect("read");
    assert_eq!(magic, [0x1f, 0x8b]);

    match load(&path) {
        Preview::Archive(archive) => {
            assert!(archive.format.contains("Tar"), "got {}", archive.format);
            assert_eq!(archive.members.len(), 1);
            assert_eq!(archive.members[0].name, "notes.txt");
            assert_eq!(archive.members[0].size, 19);
            // Tar has no per-member compression, so this must be absent rather
            // than a copy of the uncompressed size.
            assert_eq!(archive.members[0].compressed, None);
        }
        other => panic!("expected an archive, got {other:?}"),
    }
}

#[test]
fn a_pdf_renders_its_first_page() {
    let fixtures = Fixtures::new("pdf");
    let path = fixtures.write("document.pdf", &minimal_pdf());

    match load(&path) {
        #[cfg(feature = "pdf")]
        Preview::Pdf(pdf) => {
            assert_eq!(pdf.pages, 1);
            assert_eq!(pdf.page, 0);
            assert!((pdf.point_width - 200.0).abs() < 0.5);
            assert!((pdf.point_height - 100.0).abs() < 0.5);
            assert!(pdf.raster.width > 0 && pdf.raster.height > 0);
            // A page is painted white before rendering, so a fully transparent
            // result means the paint never happened.
            assert_eq!(pdf.raster.pixels[3], 255, "expected opaque paper");
            // 200×100 points is 2:1, and the rasteriser must preserve it.
            assert!((pdf.raster.aspect() - 2.0).abs() < 0.05);
        }
        other => panic!("expected a PDF, got {other:?}"),
    }
}

#[test]
fn an_out_of_range_page_fails_into_a_card() {
    let fixtures = Fixtures::new("pdf-page");
    let path = fixtures.write("document.pdf", &minimal_pdf());
    let entry = Entry::load(&path).expect("stats");

    let rendered = preview::load(
        &entry,
        Options {
            page: 9,
            ..Options::default()
        },
    );

    match rendered {
        Preview::Failed { detail, card, .. } => {
            assert!(detail.contains("page 9"), "unhelpful detail: {detail}");
            // The card is what makes a failure still a preview: it has to
            // actually describe the file.
            assert!(
                card.rows
                    .iter()
                    .any(|(field, _)| *field == meta::Field::Size)
            );
        }
        other => panic!("expected a failure card, got {other:?}"),
    }
}

#[test]
fn a_directory_is_summarised_from_its_contents() {
    let fixtures = Fixtures::new("directory");
    fixtures.write("one.txt", b"aaaa");
    fixtures.write("two.txt", b"bbbbbb");
    fixtures.write(".hidden", b"c");
    std::fs::create_dir(fixtures.path("nested")).expect("create a subdirectory");

    match load(&fixtures.0) {
        Preview::Directory(directory) => {
            assert_eq!(directory.files, 3, "hidden files still count");
            assert_eq!(directory.directories, 1);
            assert_eq!(directory.size, 11);
            assert!(!directory.unreadable);
            // The sample is for showing, so it leaves hidden entries out.
            assert!(!directory.sample.iter().any(|name| name.starts_with('.')));
            assert_eq!(directory.sample, vec!["nested", "one.txt", "two.txt"]);
        }
        other => panic!("expected a directory summary, got {other:?}"),
    }
}

#[test]
fn a_mislabelled_file_gets_the_previewer_its_bytes_deserve() {
    let fixtures = Fixtures::new("mislabelled");

    let image = image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]));
    let path = fixtures.path("definitely-a-text-file.txt");
    image
        .save_with_format(&path, image::ImageFormat::Png)
        .expect("encode");

    // The whole reason detection is content-first: the extension says text and
    // the bytes say PNG, and the bytes win.
    match load(&path) {
        Preview::Picture(picture) => {
            assert_eq!((picture.raster.width, picture.raster.height), (8, 8));
        }
        other => panic!("expected a picture, got {other:?}"),
    }
}

#[test]
fn an_unreadable_file_still_previews() {
    let fixtures = Fixtures::new("garbage");
    let path = fixtures.write("core.dump", &[0xde, 0xad, 0xbe, 0xef, 0x00, 0x01]);

    match load(&path) {
        Preview::Card(card) => {
            assert!(
                card.rows
                    .iter()
                    .any(|(field, _)| *field == meta::Field::Kind)
            );
            assert!(
                card.rows
                    .iter()
                    .any(|(field, _)| *field == meta::Field::Permissions)
            );
            assert!(card.rows.iter().any(|(field, value)| {
                *field == meta::Field::Size && *value == meta::Value::Bytes(6)
            }));
        }
        other => panic!("expected a metadata card, got {other:?}"),
    }
}

#[test]
fn the_neighbourhood_walks_the_directory_in_natural_order() {
    use peek_engine::Neighbourhood;

    let fixtures = Fixtures::new("neighbourhood");
    for name in ["img10.txt", "img2.txt", "img1.txt", ".hidden.txt"] {
        fixtures.write(name, b"x");
    }
    std::fs::create_dir(fixtures.path("subdir")).expect("create a subdirectory");

    let mut around = Neighbourhood::around(&fixtures.path("img1.txt"));

    // Hidden files and directories are excluded, and the digits sort as numbers.
    assert_eq!(around.position(), (1, 3));
    around.step(1);
    assert_eq!(
        around
            .current()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned()),
        Some("img2.txt".to_owned())
    );
    around.step(1);
    assert_eq!(
        around
            .current()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned()),
        Some("img10.txt".to_owned())
    );
    // And it wraps rather than stopping dead at the end.
    around.step(1);
    assert_eq!(around.position(), (1, 3));
}

#[test]
fn previewing_a_hidden_file_reaches_the_other_hidden_files() {
    use peek_engine::Neighbourhood;

    let fixtures = Fixtures::new("hidden");
    fixtures.write(".bashrc", b"x");
    fixtures.write(".profile", b"y");
    fixtures.write("visible.txt", b"z");

    let around = Neighbourhood::around(&fixtures.path(".bashrc"));
    assert_eq!(
        around.len(),
        3,
        "previewing a dotfile must not hide its neighbours"
    );
}

/// Build a valid single-page PDF with a correct cross-reference table.
///
/// Written by hand rather than committed as a fixture, and with real offsets
/// rather than a stub `xref`: poppler reconstructs a broken table, so a fixture
/// with wrong offsets would still open and the test would pass while proving
/// nothing about the normal path.
fn minimal_pdf() -> Vec<u8> {
    const CONTENT: &[u8] = b"BT /F1 24 Tf 20 40 Td (Peek) Tj ET\n";

    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] \
           /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", CONTENT.len()).into_bytes(),
            CONTENT.to_vec(),
            b"endstream".to_vec(),
        ]
        .concat(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];

    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());

    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    }

    let xref_at = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    // Entry zero is the head of the free list and is fixed by the spec.
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }

    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );

    pdf
}

#[test]
fn markdown_renders_rather_than_showing_its_punctuation() {
    let fixtures = Fixtures::new("markdown");
    let path = fixtures.write(
        "README.md",
        b"# Title\n\nSome **bold** prose with `code`.\n\n- one\n- two\n",
    );

    let entry = Entry::load(&path).expect("stats");
    assert_eq!(entry.kind, peek_engine::Kind::Markdown);

    match preview::load(&entry, Options::default()) {
        Preview::Text(document) => {
            assert!(document.prose, "Markdown is prose, not source");
            let text: Vec<String> = document
                .lines
                .iter()
                .map(peek_engine::text::Line::plain)
                .collect();
            let joined = text.join("\n");

            assert!(joined.contains("# Title"), "got {joined:?}");
            assert!(
                joined.contains("Some bold prose"),
                "the asterisks are rendered away, got {joined:?}"
            );
            assert!(joined.contains('\u{2022}'), "list markers are drawn");
            assert!(
                document
                    .lines
                    .iter()
                    .any(|line| line.spans.iter().any(|span| span.bold)),
                "emphasis survives as weight"
            );
        }
        other => panic!("expected rendered Markdown, got {other:?}"),
    }
}
