// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Plugins, end to end: a TOML file on disk changing what a real file
//! previews as.
//!
//! Its own test binary, and a single test, because the plugin registry is
//! loaded once per process behind a `OnceLock` — a second test running in
//! parallel would race the environment variable that points at the fixtures.

use std::path::PathBuf;

use peek_engine::{Entry, Options, Preview, preview};

struct Fixtures(PathBuf);

impl Fixtures {
    fn new() -> Self {
        let path = std::env::temp_dir().join("peek-fixtures-plugins");
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("plugins")).expect("create the fixture directory");
        Self(path)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("write a fixture");
        path
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn plugins_add_previewers_without_disturbing_the_built_in_ones() {
    let fixtures = Fixtures::new();

    // A declarative plugin claiming an extension nothing built in handles,
    // and a command plugin that renders one by shelling out.
    std::fs::write(
        fixtures.0.join("plugins/10-declarative.toml"),
        r#"
        name = "Test formats"

        [[previewer]]
        extensions = ["peekconf"]
        handler = "text"
        syntax = "TOML"
        "#,
    )
    .expect("write the plugin");

    // `cp` is the smallest honest command plugin: it is a real program, it
    // takes the placeholders positionally, and what it writes is what gets
    // shown — so a pass proves substitution, execution, and read-back.
    std::fs::write(
        fixtures.0.join("plugins/20-command.toml"),
        r#"
        name = "Test command"

        [[previewer]]
        extensions = ["peekrun"]
        handler = "command"
        command = "cp %i %o"
        output = "text"
        "#,
    )
    .expect("write the plugin");

    // Set before anything touches the registry, which is why this test is
    // alone in its binary.
    unsafe {
        std::env::set_var("PEEK_PLUGIN_DIR", fixtures.0.join("plugins"));
    }

    let load = |path: &std::path::Path| {
        let entry = Entry::load(path).expect("the fixture can be stat'd");
        (entry.kind, preview::load(&entry, Options::default()))
    };

    // A binary file with a claimed extension: it sniffs as octet-stream, so
    // nothing built in previews it, and the plugin's rule decides.
    let declared = fixtures.write("settings.peekconf", &[0x00, 0x01, b'k', b'=', b'1']);
    let (kind, previewed) = load(&declared);
    assert_eq!(
        kind,
        peek_engine::Kind::Plugin,
        "the plugin claimed the file"
    );
    match previewed {
        Preview::Text(document) => {
            assert_eq!(
                document.language.as_deref(),
                Some("TOML"),
                "the rule's declared syntax is used"
            );
        }
        other => panic!("expected the plugin's text preview, got {other:?}"),
    }

    // The command plugin: its output is what is shown.
    let run = fixtures.write("payload.peekrun", b"produced by the plugin\n");
    match load(&run).1 {
        Preview::Text(document) => {
            assert_eq!(document.lines[0].plain(), "produced by the plugin");
        }
        other => panic!("expected the command's output, got {other:?}"),
    }

    // The guarantee that makes plugins safe to install: a format peek already
    // previews is untouched, even though a plugin file is present.
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        4,
        4,
        image::Rgba([9, 9, 9, 255]),
    ))
    .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
    .expect("encode");
    let photo = fixtures.write("photo.png", &png);
    let (kind, previewed) = load(&photo);
    assert_eq!(
        kind,
        peek_engine::Kind::Image,
        "built-in detection is intact"
    );
    assert!(matches!(previewed, Preview::Picture(_)));

    // And a file no rule claims still falls through to the metadata card.
    // The null byte matters: without one these bytes sniff as text, and the
    // file would preview as text rather than reaching the card at all.
    let unknown = fixtures.write("mystery.unclaimed", &[0xde, 0xad, 0xbe, 0xef, 0x00, 0x01]);
    assert!(matches!(load(&unknown).1, Preview::Card(_)));
}
