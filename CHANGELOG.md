# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [0.1.0] — 2026-09-08

### Added

- **The engine is a published library.** `peek-engine` — type detection and
  preview rendering, with no UI dependencies — is on crates.io, so a second
  frontend links it rather than reimplementing it. Its two C-library clusters
  are behind default-on features: `pdf` (poppler, cairo) and `media`
  (GStreamer). With both off the crate builds anywhere a Rust toolchain does,
  and detection still works — `Kind` names a PDF as a PDF, and `load` falls
  back to the metadata card rather than pretending the file is unknown.
- `LICENSE`, the GPL-3.0 text the manifests have been declaring all along.
- **A plugin system.** One TOML file in `~/.local/share/peek/plugins/` adds a
  file type — either declaratively, by routing it to a previewer peek already
  has, or by naming a command that renders it. Plugins are consulted only for
  files that would otherwise show the metadata card, so installing one can add
  previews but never change or break an existing one.
- **Markdown is rendered** rather than shown as source: headings, emphasis,
  lists, quotes, tables, and syntax-highlighted code blocks, styled from the
  desktop's own syntax theme. No HTML and no browser engine.
- Fullscreen (<kbd>F</kbd> / <kbd>F11</kbd>), where the chrome yields and the
  file gets the display, and <kbd>Ctrl</kbd>+<kbd>C</kbd> to copy a path.
- An `animate` setting that turns the transitions off, for reduced motion.
- An index sheet (<kbd>G</kbd>): the whole selection as a thumbnail grid, with
  the cursor on arrows and Enter to dive in. Cells come from the same renderer
  `peek-thumbnailer` uses, so the grid and the file manager's icons cannot
  disagree.
- A settings panel in the overlay, reached from the header, so the settings no
  longer require writing RON into dotfiles.
- `just bench`, which times `preview::load` per previewer and reports the
  distribution as a table or as JSON. It reports rather than gates: decode
  time varies enough between machines that a threshold would be noise.
- Man pages for `peek` and `peek-thumbnailer`.
- `contrib/0001-cosmic-files-previewer-handoff.patch`: the cosmic-files change
  that makes the space bar reach an external previewer. Written against
  `089ad2b` and compiled there. It names the standard interface rather than
  peek, so it serves GNOME's sushi identically.
- A Flatpak manifest under `packaging/flatpak/`.
- New previewers: comics (CBZ), EPUB (cover, title, author, opening pages),
  DOCX and ODT text, and fonts as a specimen set in the font itself.
- `peek-thumbnailer`, a freedesktop thumbnailer built on the same engine, so
  the file manager's icon and the preview over it come from one decoder. It
  registers for documents, camera raw, video posters, audio cover art, and the
  image formats gdk-pixbuf installations commonly lack.
- A CI workflow: formatting, pedantic clippy, the test suite, and the desktop
  metadata validators on every push.
- Animated GIF, APNG, and animated WebP now play, at the frame timing browsers
  use; the header notes when an image is animated.
- Camera raw files (CR2, NEF, ARW, DNG, RAF, ORF, RW2, PEF and more) preview
  via the full-size JPEG the camera embedded, with EXIF orientation applied.
- The pointer joins the keyboard: scroll to zoom, drag to pan a cropped image,
  double-click to jump between the fitted view and 100%. Clicking the content
  no longer dismisses the preview; clicking outside the panel still does.
- Adjacent files decode ahead of the arrow keys, so holding one lands on
  finished previews instead of starting decodes.

### Changed

- Remote URIs (`smb://`, `sftp://`) now preview when GVFS has already mounted
  them, resolved through GIO. `peek` still never triggers a mount.
- Prose and source are now typeset differently: Markdown and extracted
  document text wrap in a proportional face with no line numbers, while source
  stays monospaced, numbered, and windowed.
- Text previews render a window around the scroll position instead of shaping
  the whole document, lifting the line ceiling from 4 000 to 100 000. Syntax
  highlighting is bounded by time rather than cut off: a pathological file
  continues as plain text instead of ending early.
- A short text file gets a panel sized to its content instead of floating in a
  full-height reading column.
- PDF documents are parsed once and held open on a worker thread; page turns
  cost only the rendering.
- Every string the overlay shows is now translatable: MIME type names and
  decode-failure reasons moved behind the Fluent catalogue. The technical
  error text still reaches the journal and `probe`.

[0.1.0]: https://github.com/entro314-labs/peek/releases/tag/v0.1.0
