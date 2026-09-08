# peek

A QuickLook-class file previewer for the COSMIC desktop.

`peek` is a `wlr-layer-shell` overlay built on libcosmic 1.0 / iced 0.14. Point
it at a file and it shows you the file — the picture, the pages, the video, the
source with its syntax coloured, the contents of the archive — without opening an
application. Arrow keys walk the rest of the directory. Space closes it.

Same architecture as [`light`](https://github.com/entro314-labs/light): a
resident daemon, a layer surface created on demand, settings in `cosmic-config`,
and an engine crate with no UI dependencies so a second frontend can share it.

## Status

Working and dogfoodable on COSMIC 1.5.

- Overlay maps on the overlay layer above the panel and fullscreen windows
- Exclusive keyboard focus; space, escape, arrows, zoom, transport
- Images (PNG, JPEG, WebP, AVIF, TIFF, HDR, EXR, QOI, …) with EXIF orientation
  applied and camera details shown
- Animated GIF, APNG, and WebP actually animate, at the frame timing browsers
  use
- Camera raw (CR2, NEF, ARW, DNG, RAF, ORF, RW2, PEF) via the full-size JPEG
  the camera embedded — the same rendition Quick Look shows
- SVG rendered at display size, not scaled from a thumbnail
- PDF with page navigation, through poppler; the document is parsed once and
  held open, so page turns cost only the rendering
- Source and text with syntax highlighting in COSMIC's own palette, light and
  dark, over `two-face`'s syntax set — the same pair cosmic-edit uses; long
  files render windowed, so a 100 000-line log scrolls without shaping it all
- Video and audio with real playback, seeking, and a poster frame
- Comics (CBZ) paged like a document, and EPUB with its cover, bibliography,
  and opening pages
- DOCX and ODT reduced to their text — what the file *contains*, without
  embedding an office suite
- Fonts as a specimen, set in the font itself
- Archives listed without extracting: zip, 7z, tar and its four compressions
- Directories summarised; anything else gets a metadata card
- Zoom with the scroll wheel, pan a cropped image by dragging, double-click to
  jump between fit and 100%; fullscreen when the file should have the display
- Rendered Markdown — headings, emphasis, lists, and highlighted code blocks,
  with no browser engine anywhere near the process
- A plugin system: one TOML file adds a file type, either by routing it to a
  previewer that already exists or by naming a command that renders it. See
  [res/plugins/README.md](res/plugins/README.md)
- Adjacent files decode ahead of the arrow keys, so holding one lands on
  finished previews
- `peek-thumbnailer`, a freedesktop thumbnailer on the same decoders, so the
  file manager's icon and the preview over it cannot disagree
- An index sheet (<kbd>G</kbd>): the whole selection as a thumbnail grid,
  rendered by the same code the file manager's icons come from
- A settings panel in the overlay itself, so the settings are reachable
  without writing RON into dotfiles
- `org.gnome.NautilusPreviewer2`, so the space bar works in Nautilus today
- Compositor backdrop blur via `ext-background-effect-v1`

Not built yet: 3D models, colour management, and the GTK4/GNOME frontend. The path from here to Quick Look parity is laid out in
[ROADMAP.md](ROADMAP.md).

## The space bar

This is the part of QuickLook that is not about rendering files, and it is worth
being precise about.

Pressing space in a file manager previews *the selection*, and the only program
that knows what is selected is the file manager. So the previewer cannot
implement the gesture on its own — something has to call it.

On Linux exactly one interface does this: **`org.gnome.NautilusPreviewer2`**.
Nautilus calls `ShowFile` when space is pressed, `Close` when the preview should
go, and listens for a `SelectionEvent` signal so arrow keys pressed *inside* the
preview move its selection — which produces another `ShowFile`. That round trip
is the whole gesture. `peek` serves that interface, so **in Nautilus the space
bar works with no further setup**.

GNOME's own previewer, `sushi`, claims the same bus name. `peek` asks to replace
it and hands it back on exit, so whichever started last wins. To make the
takeover survive a reboot:

```sh
systemctl --user mask --now org.gnome.NautilusPreviewer.service
```

**cosmic-files already binds space — to its own internal viewer.** In
[`src/key_bind.rs`](https://github.com/pop-os/cosmic-files/blob/master/src/key_bind.rs)
it binds plain <kbd>Space</kbd> to `Gallery` and <kbd>Ctrl</kbd>+<kbd>Space</kbd>
to `Preview` (the side pane). The gesture exists; it just never leaves the
process. Two consequences:

- Its `Message::GalleryToggle` handler only fires for items where
  `can_gallery()` is true, and that is **images and text only**. Press space on a
  PDF, a video, an audio file, or an archive in cosmic-files and nothing happens
  at all.
- The bindings are not configurable — `key_bind.rs` carries a literal
  `//TODO: load from config` — so this cannot be redirected from the outside,
  and nothing else exposes the selection to another process.

So on COSMIC, `peek` is reached three ways today — the command line, its D-Bus
activation, and **Open With → Peek**. The fourth needs a change to cosmic-files,
and that change is written:
[`contrib/0001-cosmic-files-previewer-handoff.patch`](contrib/0001-cosmic-files-previewer-handoff.patch).
In the `GalleryToggle` handler, when nothing in the selection is something
`can_gallery()` accepts, it calls `ShowFile` on the previewer interface. Images
and text keep the built-in gallery; everything else stops doing nothing.

Framed that way it is not "make cosmic-files use `peek`" — it is "make space work
for the file types the built-in gallery does not handle, through the standard
desktop interface", which also works for anyone running GNOME's sushi. That is
the version worth sending upstream, and it is the version in the patch: it
names no previewer, only the interface. It applies to `089ad2b` (Epoch 1.8.0)
and compiles there.

## Install

```sh
just build-release
just install-user      # or `sudo just install` for the whole system
```

`just install-user` puts the binary in `~/.local/bin`, the desktop entry in
`~/.local/share/applications`, the icon in `~/.local/share/icons`, and the
AppStream metadata in `~/.local/share/metainfo`, then refreshes both caches.
`just uninstall` takes them out again, and `just validate` runs
`desktop-file-validate` and `appstreamcli` over the two metadata files.

Requires poppler-glib, GStreamer with the plugin sets you want to decode, and a
`shared-mime-info` database — all of which a COSMIC install already has.

## Use

```sh
peek photo.jpg          # preview it; arrows walk the rest of the directory
peek *.png              # preview the set; arrows walk exactly these
peek                    # toggle the last preview
```

The first invocation becomes the daemon and stays resident. A second invocation
reaches it over D-Bus, which is what makes `peek` itself the right thing to bind
to a shortcut:

**Settings → Desktop → Keyboard → Keyboard Shortcuts → Custom Shortcuts**, with
the command `peek` (absolute path until it is on `$PATH`).

`just bench` times the path between pressing space and seeing the file, per
previewer; `man peek` documents the rest.

`RUST_LOG=peek=debug` for logs. They go to the journal — `journalctl --user -t
peek -f` — and to stderr only when there is no journal to write to, because a
daemon started by D-Bus activation has a stderr nobody can see.

### Keys

| Key | Does |
| --- | --- |
| <kbd>Space</kbd>, <kbd>Esc</kbd> | Close |
| <kbd>←</kbd> <kbd>→</kbd> | Previous and next file |
| <kbd>↑</kbd> <kbd>↓</kbd> | Previous and next page — or file, where there are no pages |
| <kbd>Shift</kbd>+<kbd>←</kbd>/<kbd>→</kbd> | Seek five seconds |
| <kbd>P</kbd> | Play and pause |
| <kbd>+</kbd> <kbd>-</kbd> <kbd>0</kbd> | Zoom in, out, reset |
| <kbd>F</kbd>, <kbd>F11</kbd> | Fullscreen — the file gets the display |
| <kbd>G</kbd> | The index sheet: every file in the selection, at once |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> | Copy the file's path |
| <kbd>Enter</kbd> | Open in the default application |

<kbd>Space</kbd> and <kbd>Esc</kbd> mean "back": they leave fullscreen if it is
on, and close the preview otherwise.

The pointer works too: scrolling over an image, page, or video zooms it,
dragging pans it once zoom has cropped it, double-clicking jumps between the
fitted view and the file's own pixels, and clicking outside the panel
dismisses — clicking the content itself never does.

## Why it is built this way

**Detection is content-first.** A previewer is pointed at whatever the cursor
happens to be on, and file managers routinely show `.txt` files that are really
PNGs and extensionless files that are really source. The MIME type comes from the
system `shared-mime-info` database — the same one the file manager consults — so
the two cannot disagree about what a file is while both are on screen. The
extension is only consulted for files that already sniffed as text, where it is
the sole evidence for *which* syntax.

**Failure is a preview, not an error.** `peek_engine::preview::load` has no error
type. A corrupt JPEG, an encrypted PDF, and a damaged archive all resolve to the
metadata card with the reason attached. The user pressed a key over a file that
exists; showing them its size, type, and modification date plus "could not
decode" is strictly better than an empty panel — and it means the overlay has no
empty state to design.

**The panel is shaped by the file.** A launcher's panel is a fixed-width list. A
previewer's is whatever shape the content is, so
[`surface::panel_rect`](src/surface.rs) derives its geometry from the preview's
aspect ratio. Without that, a portrait photograph opens in a landscape frame with
bars down both sides, which is the single most visible way a previewer looks
wrong.

**Loads are generation-stamped.** Decoding runs on a blocking thread and takes
between a millisecond and most of a second. Holding the right arrow starts
several loads before the first finishes, and they do not finish in order. Every
load carries the generation it began at and a stale result is dropped — the same
rule `light` applies to search responses.

**A text document is one widget.** A highlighted file is thousands of lines of
several coloured runs each; a widget per run means tens of thousands of nodes in
a scrollable that lays out all of its children. The whole document becomes a
single `rich_text` whose spans carry the newlines, so the shaper sees one
paragraph and the widget tree sees one node. That is also why the document limit
is four thousand lines: the bound is the *renderer's* shaping cost, not syntect's.

**Archives are listed, never extracted.** A previewer that unpacks an archive to
show you its contents has written to a directory the user did not choose, with
collisions they cannot see, and has to clean up afterwards — which is the part
that goes wrong.

**Image handles are built once.** `Handle::from_rgba` assigns a fresh id on
every call, so a handle built in `view` misses the renderer's texture cache and
re-uploads the whole image every frame. They are built when a preview lands and
when a video frame arrives, and nowhere else. The video path hands its buffer
over rather than cloning it, which is the difference between copying eight
megabytes per frame and copying none.

**It is a daemon.** Creating a wgpu device, deserialising the syntax set,
and building GStreamer's registry all happen once at startup. The daemon is also
what keeps the previewer service on the bus for the file manager to find, which
is why `peek` with no arguments stays running instead of exiting.

## Configuration

The gear in the overlay's header opens a settings panel; everything below can
also be set from the command line. Settings live in `cosmic-config` under
`~/.config/cosmic/dev.entro314labs.Peek/v1/`, one RON file per key, applied live
either way.

```sh
echo -n 'video-only' > ~/.config/cosmic/dev.entro314labs.Peek/v1/autoplay
echo -n 'false' > ~/.config/cosmic/dev.entro314labs.Peek/v1/blur
```

| Key | Default | Does |
| --- | --- | --- |
| `blur` | `true` | Frosted backdrop behind the panel |
| `opacity` | `0.86` | Panel fill, clamped to 0.4–1.0 |
| `autoplay` | `always` | `always`, `video-only`, or `never` |
| `loop_media` | `false` | Restart at the end |
| `line_numbers` | `true` | Numbers beside text previews |
| `max_fraction` | `0.86` | Largest share of the display the panel takes |
| `nautilus_previewer` | `true` | Serve `org.gnome.NautilusPreviewer2` |
| `follow_selection` | `true` | Move the file manager's cursor with the arrows |
| `click_away` | `true` | Click outside the panel to dismiss |
| `animate` | `true` | Play the open, close, and step transitions |

An unset key uses its default; an unreadable one logs and falls back, so a bad
value cannot stop a preview opening.

## Layout

```
i18n/en/peek.ftl       every string the interface shows
res/                   desktop entry, AppStream metadata, icon
crates/peek-engine/    engine, no UI dependencies; the published crate
  kind.rs              content-first type detection
  entry.rs             the file, and the files either side of it
  picture.rs           raster decoding, animation frames, SVG rendering
  raw.rs               camera raw, via the embedded JPEG
  pdf.rs               poppler page rasterisation, and the held-open session
  text.rs              syntect highlighting into plain spans
  archive.rs           zip / 7z / tar listing
  markdown.rs          Markdown rendered into spans, never into HTML
  plugin.rs            third-party previewers, declared in TOML
  comic.rs             CBZ pages, in natural order
  ebook.rs             EPUB cover, metadata, and opening text
  office.rs            DOCX / ODT text extraction
  font.rs              font metadata for the specimen
  media.rs             GStreamer probe, poster frame, and playback
  meta.rs              the metadata card and directory summary
  thumbnail.rs         one image per file, for the grid and the thumbnailer
  preview.rs           choosing a previewer, and failing into a card
  examples/probe.rs    report what a file decodes to, without a window
  themes/              COSMIC's syntax palettes, vendored as tmTheme
crates/peek-thumbnailer/
                       freedesktop thumbnailer on the same engine
contrib/               the cosmic-files patch that makes space reach peek
packaging/flatpak/     the Flatpak manifest
res/plugins/           how to write a plugin, and two worked examples
src/                   the COSMIC frontend
  surface.rs           layer-shell surface, geometry, blur region
  fonts.rs             registering a previewed font with the renderer
  anim.rs              open transition and step cross-fade
  view.rs              rendering, and every user-visible string
  localize.rs          Fluent catalogues and the `fl!` macro
  previewer.rs         org.gnome.NautilusPreviewer2
  app.rs               state and update loop
```

The engine reports facts, not sentences: a metadata row carries a
`meta::Field` and a `meta::Value` rather than two `String`s, a modification
time arrives as a `meta::Elapsed` bucket rather than as "3 hours ago", and a
failed decode arrives as a `preview::Reason` rather than as an error message.
The wording is chosen in `view.rs`, which is the only layer that knows what
language the user reads. The `thiserror` messages still exist, but they go to
the journal and the `probe` example — nothing the overlay shows escapes the
Fluent catalogue.

Unlike `light`, the engine is **not** MPL-2.0. It links poppler, and poppler is
GPL, so the whole workspace is GPL-3.0-or-later. A GTK4 frontend would be linking
the same poppler, so this costs nothing.

## Known rough edges

- The space bar does not reach `peek` from cosmic-files. See above — this needs a
  change upstream, not here.
- The desktop entry's `Name`, `Comment` and `Keywords` are not translated.
  cosmic-app-template generates the localised variants with an `xdgen` build
  script, but adding that build dependency here re-resolves the dependency graph
  in a way that drops zbus's `blocking` feature and breaks libcosmic's
  `single-instance` support.
- Decoding is deferred until the surface has been configured. Without that, the
  very first preview of a *video* produced a surface that was created, reported
  as mapped, and never drawn — the panel sat at zero opacity while the
  application believed it was visible. Starting a GStreamer pipeline while wgpu
  is still building its device on the same GPU is the suspected cause; the
  deferral avoids the race rather than explaining it, so treat this as worked
  around rather than fully understood.
- Text previews do not wrap long lines; what runs past the column is clipped.
  The trade bought exact line-height arithmetic, which is what lets a
  100 000-line file render windowed and keeps the line numbers honest — a
  horizontal scroll is the intended fix, not re-enabling wrap.
- `SelectionEvent`'s declared signature disagrees with what Nautilus parses:
  sushi's introspection says `q` (u16), Nautilus reads `(u)` (u32). `peek` emits
  `u`, matching the consumer. If a future Nautilus switches, the direction will
  be misread and its cursor will stop following — the previewer's own arrow keys
  are unaffected either way.
- CBR (RAR-compressed comics) is not read; only CBZ is. RAR needs a decoder
  this workspace does not carry.
- WOFF and WOFF2 get the metadata card rather than a specimen: `ttf-parser`
  does not unpack their compression, and detection keeps them away from the
  previewer rather than promising a specimen that would fail.
- Office previews are text, not layout. Tables, images, and formatting are not
  shown — embedding LibreOffice to do so is an explicit non-goal.
- CR3 gets a failure card rather than its preview: Canon moved to an ISO BMFF
  container, which the raw previewer's TIFF walk cannot read. The other raw
  formats work; this one needs its own parser.
- Remote URIs (`smb://`, `sftp://`) preview only when GVFS has *already*
  mounted them; `peek` never triggers a mount. A previewer is pointed at
  whatever the cursor is over, and a keypress that opens a network connection
  and blocks on a server is not a preview.
