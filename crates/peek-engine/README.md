# peek-engine

Type detection and preview rendering, with no UI dependencies.

This is the engine behind [`peek`](https://github.com/entro314-labs/peek), a
QuickLook-class file previewer for the COSMIC desktop. Everything a previewer
frontend needs that is not drawing: deciding what a file is, turning it into
pixels or lines or a listing, and describing it when none of those apply.

It is a separate crate because a second frontend — a GTK4 one, for the desktops
where `wlr-layer-shell` is not available — should link this rather than
reimplement it. `peek-thumbnailer`, the freedesktop thumbnailer that ships
alongside the app, is the second consumer today: the file manager's icon and the
preview over it come from the same decoders and so cannot disagree.

```rust,no_run
use peek_engine::{Entry, Neighbourhood, Options, Preview, preview};

let mut around = Neighbourhood::around(std::path::Path::new("/tmp/photo.jpg"));
let entry = Entry::load(around.current().expect("a current file")).expect("stats");

// Blocking: run this off the frame path.
match preview::load(&entry, Options::default()) {
    Preview::Picture(picture) => {
        let _ = (picture.raster.width, picture.raster.height);
    }
    Preview::Failed { detail, .. } => eprintln!("{detail}"),
    _ => {}
}

around.step(1); // right arrow
```

## What it previews

Images (including animation and EXIF orientation), camera raw, SVG, PDF, comics
(CBZ), EPUB, DOCX and ODT text, fonts as a specimen, source and text with syntax
highlighting, rendered Markdown, video and audio, archives listed without
extracting, and directories. Anything else resolves to a metadata card.

Detection is content-first, extension-second, against the system's
`shared-mime-info` database — the same one the file manager consults, so the two
cannot disagree about what a file *is* while both are on screen.

## Failure is a preview, not an error

`preview::load` has no error type. A corrupt JPEG, a password-protected PDF, and
an archive with a damaged header all resolve to `Preview::Failed`, which carries
the metadata card alongside a `Reason`. The engine knows *which* failure
happened; the frontend chooses the words, because it is the only layer that
knows what language the user reads.

## Features

Two clusters of dependency are C libraries with system packages behind them, and
they are the only reason building this crate can fail on a machine that has a
Rust toolchain and nothing else. Both are on by default and both can be turned
off.

| Feature | Default | Pulls in | Needs |
| --- | --- | --- | --- |
| `pdf` | yes | `poppler-rs`, `cairo-rs`, `glib` | `libpoppler-glib-dev`, `libcairo2-dev` |
| `media` | yes | `gstreamer` and friends | `libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev` |

```toml
# Type detection, images, text, archives — no C libraries, builds anywhere.
peek-engine = { version = "0.1", default-features = false }
```

Detection is deliberately *not* gated. `Kind` still names a PDF as a PDF with
`pdf` off, and `load` falls back to the metadata card — so a caller that matches
on `Kind` keeps working either way, and a cut-down build never claims a PDF is an
unknown blob.

## Licence

GPL-3.0-or-later. Poppler, the only PDF renderer actually shipped on a Linux
desktop, is GPL, and that decides the licence for the whole workspace.

The two `.tmTheme` files under `themes/` are MPL-2.0 and carry their own
attribution — see [`themes/README.md`](themes/README.md).
