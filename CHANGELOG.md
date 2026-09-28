# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- The settings panel has a switch for answering the space bar in Nautilus
  (`nautilus_previewer`), which could only be set by editing the config
  file.

### Changed

- `.tar.xz` archives are listed by a pure-Rust decoder, so the packages no
  longer depend on `xz` / `liblzma`. A stream whose header asks for more than
  256 MiB of decoder memory is refused instead of allocated.

### Fixed

- A comic book (CBZ) whose page claims to be small but inflates to gigabytes
  is refused instead of being read into memory. Before, such a file could
  exhaust memory in the previewer or in `peek-thumbnailer` when a file manager
  thumbnailed its folder.
- The index sheet (<kbd>G</kbd>) is drawn in the same column the arrow keys
  and the backdrop blur assume, so <kbd>↑</kbd>/<kbd>↓</kbd> move by one
  visible row.
- A command plugin receives the right file when its name contains `%o`, `%s`
  or `%i`; the placeholders are no longer substituted inside the file name.
- Word documents with custom tab stops no longer preview with stray tabs at
  the start of each paragraph.
- Text and Markdown files over 4 MB are no longer reported as containing
  invalid text when the preview limit falls inside a multi-byte character.
- `nautilus_previewer = false` is honoured: Peek no longer takes
  `org.gnome.NautilusPreviewer` from GNOME's previewer when the setting is
  off, and turning it off while Peek is running gives the name back at once.
- Arrowing onto a file that was deleted after it was selected stops the
  previous file's audio or video and says the file no longer exists, under
  its name, instead of showing an empty card while the old track plays on.
- Opening the index sheet on a large folder decodes at most one thumbnail
  per processor core at a time. Before, every cell (up to 250) started
  decoding at once, which could hold gigabytes of image buffers in a camera
  folder.
- A compressed file that is not a tarball (`access.log.gz`, a `.xz` disk
  image) shows its details instead of "The file could not be decoded", and
  so do RAR, ISO, `.deb` and `.rpm` files, which Peek does not list. Those
  four can now be claimed by a plugin.
- `.tar.lzma` archives are listed. They are LZMA-alone streams, not xz, and
  were handed to the xz decoder, which rejects them.
- Comic pages are rendered again at full resolution when the overlay learns
  the display's scale factor or size, instead of staying at the 1× size on a
  HiDPI display. Photos are no longer decoded a second time when that
  happens, which also stopped animated images restarting.
- Changing the accent colour or the frosted setting no longer restarts a
  playing video or re-decodes the file on screen. Switching between light
  and dark re-highlights source text, and nothing else.
- Changing the largest panel size in the settings file re-renders the SVG
  or page on screen, the way moving the slider in the panel already did.
- Video posters are scaled to at most 1600 pixels on their longer edge, as
  intended: a 4K video's poster was the full 33 MB frame and an 8K video had
  none. Anamorphic video (a DVD's 720 stored columns shown at 16:9) gets a
  poster in its displayed shape.
- Matroska files preview and play. `.mkv` and `.mka` were detected as
  `application/x-matroska`, which nothing claimed, so they showed only the
  metadata card.

## [2.1.0] - 2026-09-22

### Changed

- Rebuilt against the current COSMIC libraries (libcosmic `03c8f93`).

## [2.0.0] - 2026-09-21

### Changed

- **The command and the package are `magnetar-peek`.** Arch, Debian and Fedora
  all ship an unrelated `peek` (a GIF recorder, 1.5.x) under that package and
  command name. Sharing it meant `pacman -S peek` installed the other program,
  `pacman -Syu` replaced this one with it, and the two could not be installed
  together. The package replaces this project's own `peek` 1.0.1 and earlier
  on upgrade and leaves the GIF recorder alone. A keyboard shortcut bound to
  `peek` needs changing to `magnetar-peek`; logs are now under
  `journalctl --user -t magnetar-peek`.

### Fixed

- The package declares `xz`, which the binaries link directly. It was
  already present on any Arch system, so nothing failed to start.

## [1.0.1] - 2026-09-16

### Fixed

- The Arch package is valid. The v1.0.0 pacman package carried a tar entry
  with an empty name, so the repository refused it and Peek never reached
  `pacman -S`. The package is now built one directory tree at a time.

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

[Unreleased]: https://github.com/Magnetar-OS/peek/compare/v2.1.0...HEAD
[2.1.0]: https://github.com/Magnetar-OS/peek/compare/v2.0.0...v2.1.0
[2.0.0]: https://github.com/Magnetar-OS/peek/compare/v1.0.1...v2.0.0
[1.0.1]: https://github.com/Magnetar-OS/peek/compare/v0.1.0...v1.0.1
[0.1.0]: https://github.com/Magnetar-OS/peek/releases/tag/v0.1.0
