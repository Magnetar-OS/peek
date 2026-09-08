# Roadmap

The destination is fixed: a previewer that can stand 1:1 against **macOS Quick
Look** — the application that defines this category — while being a first-class
COSMIC citizen rather than a port of someone else's idea. Where Quick Look is
vague or macOS-specific, the secondary references are **GNOME Sushi** (the Linux
incumbent) and **QuickLook for Windows** (which shows what a plugin-driven
previewer grows into).

The measure for every milestone is the same one the release profile already
optimises for: the delay between pressing space and seeing the file, and
whether what appears looks like it belongs on this desktop.

## Where Peek stands against Quick Look

| Capability | Quick Look | Sushi | Peek today |
| --- | --- | --- | --- |
| Space-bar preview from the file manager | yes | yes | Nautilus yes; cosmic-files needs the upstream patch |
| Arrow-key navigation through the selection | yes | yes | yes |
| Images with EXIF orientation and camera details | yes | yes | yes |
| Animated images (GIF, APNG, animated WebP) | yes | yes | **yes** |
| RAW camera formats | yes | no | **yes**, via the embedded JPEG (not CR3) |
| SVG at display size | yes | yes | yes |
| PDF with page navigation | yes | yes | yes, document held open |
| Text and source with highlighting | yes | yes | **yes**, windowed to 100 000 lines |
| Text selection and copy inside the preview | yes | yes | no |
| Video and audio with scrubbing | yes | yes | yes |
| Archives listed without extraction | yes | yes | yes |
| Font specimen | yes | yes | **yes** (not WOFF/WOFF2) |
| EPUB / ebooks | yes | yes (evince) | **yes**: cover, metadata, opening pages |
| Comics (CBZ) | yes | no | **yes** (not CBR) |
| Office documents | yes | no | **text only**, by design |
| 3D models | yes (USDZ) | no | metadata card |
| Fullscreen / slideshow mode | yes | no | no |
| Multi-selection index sheet (grid) | yes | no | **yes** |
| Pinch zoom, pan, trackpad gestures | yes | partial | **scroll, drag, double-click** |
| Markdown rendered | yes | yes | **yes** |
| Fullscreen | yes | no | **yes** |
| Settings interface | yes | no | **yes** |
| Third-party previewers (plugins) | yes (Quick Look generators) | no | **yes**, TOML + subprocess |
| Thumbnails for the file manager | yes (same framework) | no (separate) | **yes**, same engine |
| Open in default application | yes | yes | yes |
| Metadata inspector | yes | no | yes |

Everything below exists to close that table, in an order where each milestone
is independently shippable and nothing waits on anything it does not need.

## 0.2 — Finish what is on screen ✅ *landed*

Parity *within* the previewers that already exist, and the rough edges the
README admits to. No new file types.

- **Animated images play.** GIF, APNG, and animated WebP decode as frame
  sequences and play through the same handle-built-once path the video poster
  uses. A previewer that freezes a GIF reads as broken to anyone under thirty.
- **Zoom gets a body.** Pan when zoomed in (drag and arrow-scroll), scroll-wheel
  and pinch zoom on the image, double-click to toggle fit/100%. The keyboard
  bindings stay; they stop being the only way.
- **Adjacent entries preload.** Holding an arrow through a directory should hit
  a decoded neighbour, not start a decode. The generation-stamping rule already
  handles the cancellation half; this adds the speculative half (n±1, bounded
  memory, dropped on direction change).
- **PDF stops reopening per page.** `poppler::Document` is not `Send`, so it
  cannot sit in application state — but it can live on a dedicated worker
  thread that owns it and answers page requests over a channel. One parse per
  document, and page-turn latency becomes render cost alone.
- **RAW camera formats show the photograph.** CR2, NEF, ARW carry a full-size
  embedded JPEG preview; extracting it is cheap and is what Quick Look itself
  shows. Full RAW decode is explicitly *not* the goal — the embedded preview is.
- **The text ceiling becomes a window.** The 4 000-line limit is the renderer's
  shaping cost, so the fix is shaping less: highlight the whole file (syntect is
  not the bottleneck) but hand the widget a window of it that follows the
  scroll. The fixed-height reading column gets a floor instead, so a three-line
  file no longer floats in an empty panel.
- **The last untranslated strings make the trip.** `meta::describe_mime` and the
  `thiserror` failure messages move behind Fluent, closing the two gaps the
  README documents. The desktop entry's `Name[xx]=` lines stay hand-maintained
  until the xdgen/zbus feature-resolution conflict is fixed upstream.
- **The video-surface race gets understood.** The configure-before-decode
  deferral works; 0.2 either explains it (and possibly removes it) or documents
  the mechanism precisely enough that it is a decision rather than a
  workaround.

## 0.3 — The space bar everywhere ✅ *landed*

Peek's rendering is only half the product; the other half is being reachable
from wherever the user's cursor is. This milestone is integration, not pixels.

- **The cosmic-files patch goes upstream.** *Written and compiled* against
  `089ad2b`, as `contrib/0001-cosmic-files-previewer-handoff.patch`; sending
  it is all that remains, and that is not a code task. As framed in the README: when
  `GalleryToggle` fires on an item `can_gallery()` rejects, call `ShowFile` on
  `org.gnome.NautilusPreviewer2`. Images and text keep the built-in gallery;
  PDFs, video, audio, and archives stop doing nothing — and the patch serves
  sushi users identically, which is what makes it upstreamable.
- **A thumbnailer built on the same engine.** A `peek-thumbnailer` binary
  implementing the freedesktop thumbnail spec, using `peek-engine`'s decoders —
  so the icon in the file manager and the preview over it can never disagree
  about what a file looks like. This is Quick Look's actual architecture: one
  framework feeding both Finder's thumbnails and the space bar. `blake3` is
  already in the tree for the cache keys.
- **Settings grow an interface.** *Landed*, as a panel inside the overlay
  rather than a context drawer — the overlay has no header bar to hang one
  from. The nine `cosmic-config` keys are currently
  set by echoing RON at dotfiles. A settings view — libcosmic context drawer,
  the pattern every COSMIC applet uses — makes them discoverable. The config
  is already watched, so changes keep applying live.
- **Flatpak, and the stores.** *Manifest written* at
  `packaging/flatpak/`, with the sandbox trade-offs argued in its comments; it
  needs a real poppler checksum and a Flathub submission. `nfpm` covers deb and rpm; a Flatpak
  manifest brings Flathub and the COSMIC Store. The layer-shell surface and
  the D-Bus names need portal-era scrutiny here — this is where sandboxing
  assumptions get found, not papered over.

## 0.4 — More of the world's files ✅ *landed*

New previewers, every one of them in `peek-engine` behind the same contract:
detection is content-first, failure resolves to the metadata card, strings are
facts not sentences, and a fixture test lands in `tests/previews.rs` with it.

- **Fonts.** A specimen card for TTF, OTF, and WOFF2: the family and style
  names from the name table, a pangram at several sizes, the glyph coverage
  summarised. Sushi has had this for a decade; it is table stakes.
- **Ebooks.** EPUB is a zip of XHTML — the archive code and the text code
  already exist. Cover, title, author, and readable chapter text through the
  same reading column source files use.
- **Comics.** CBZ and CBR are archives of images; page navigation is the PDF
  interaction. Nearly free given what is already built, and beloved by the
  people who have it.
- **Markdown, rendered.** *Landed.* Source view is honest but wrong for prose;
  `pulldown-cmark` into the existing span machinery renders headings, emphasis,
  lists, and code blocks without a webview anywhere near the process.
- **Office documents, honestly.** DOCX and ODT are zipped XML; extracting the
  text and showing it in the reading column covers the "what is in this file"
  question, which is the previewer's question. Full-fidelity layout would mean
  embedding LibreOffice, and that is a non-goal — the card says what the file
  is, the text says what it contains.
- **Remote URIs, decided.** *Landed*: resolved through GIO when GVFS has
  already mounted them, never triggering a mount. `smb://` and `sftp://` were declined.
  Either resolve them through an existing GVFS mount when one is present
  (never triggering a mount), or keep declining — but as a documented decision
  with the reasoning, not a rough edge.

HTML stays a source preview. Rendering HTML means shipping a browser engine,
and every previewer that did so came to regret it; the syntax-highlighted
source plus metadata card is the honest version.

## 0.5 — The gallery

Quick Look is not one surface; it is a small family of them. This milestone
adds the other members, all on the same layer-shell + wgpu foundation.

- **The index sheet.** *Landed* (<kbd>G</kbd>). Cells are rendered by
  `peek_engine::thumbnail`, the same call `peek-thumbnailer` answers the file
  manager with, so the two cannot drift. Bounded to 250 rendered cells; past
  that the grid still lists every file, with type icons.
- **Fullscreen.** *Landed* — <kbd>F</kbd> drops the header and the panel's own
  surface and gives the content the output. The optional slideshow timer is
  not built.
- **The filmstrip.** *Still open.* A strip of neighbour thumbnails along the
  bottom edge while walking a directory. The index sheet now supplies both the
  renderer and the cache it would draw from, so what remains is the strip
  itself and deciding whether it earns its vertical space beside the sheet.
- **Actions, within reason.** *Copy path landed* (<kbd>Ctrl</kbd>+<kbd>C</kbd>).
  An Open With… submenu is still open, and copying the *file* needs a richer
  clipboard than `clipboard::write`'s text. Editing — markup, trim, rotate —
  is deliberately out: Peek is a viewer, and the moment it writes to the file
  it is something else.

## 0.6 — wgpu earns its keep

The renderer requirement stops being infrastructure and becomes features.

- **3D models.** glTF/GLB, STL, and OBJ on a turntable, rendered by the same
  wgpu device the overlay already owns. This is Quick Look's USDZ story with
  the formats Linux users actually have, and no other Linux previewer has it.
- **Color, managed.** ICC profiles honoured on decode; wide-gamut and HDR
  images tone-mapped deliberately instead of clipped, and passed through
  untouched once the compositor's color-management protocol support lands in
  libcosmic. The EXR and HDR decoders are already in the tree; today their
  output is only accidentally correct.
- **Motion, polished.** The animation layer now sits behind an `animate`
  setting, so reduced motion is available today. It is a *peek* setting rather
  than the system's because COSMIC publishes no reduced-motion preference yet;
  when it does, `animate` becomes the opt-out from it, the way `blur` already
  relates to the desktop's frosted setting. The high-refresh-rate audit is
  still open.

## 1.0 — Parity, audited

1.0 is not a feature; it is the claim that the table at the top of this file
has no cell reading worse than Quick Look's, verified rather than believed.

- **Accessibility, verified.** The `a11y` feature flag is on; 1.0 means the
  overlay has actually been driven with AccessKit tooling and a screen reader,
  and every control announces itself.
- **Performance, budgeted.** *Harness landed* as `just bench`: fixtures per
  previewer, best/median/worst, table or JSON. It deliberately does not gate
  in CI — decode time varies by an order of magnitude between machines, so a
  threshold tight enough to catch a regression would fail constantly. Its
  first finding is that syntax highlighting dominates everything: a
  2 000-line Rust file costs about four times a 12-megapixel JPEG.
- **The test matrix filled in.** Every `Kind` has fixtures in
  `tests/previews.rs`, including the failure fixtures — the corrupt JPEG and
  encrypted PDF the README promises resolve to cards.
- **CI beyond release.** The boring workflow — fmt, clippy, tests,
  `just validate` — landed early, in 0.3. What remains for 1.0 is pinning the
  release-kit references to reviewed SHAs, as the TODO in `release.yml`
  already demands.
- **i18n complete**, including the desktop entry, however the xdgen conflict
  resolves.
- **Documentation for humans.** *Man pages landed* for `peek` and
  `peek-thumbnailer`. Screenshots in the AppStream metadata still need a
  running COSMIC session to capture, which is not something a build can do.

## Beyond 1.0

Two ideas that are real but must not distort the architecture before it:

- **The GTK4/GNOME frontend.** The reason `peek-engine` has no UI dependencies.
  It stays on the roadmap as the proof of the engine split — but it ships
  after the COSMIC frontend is finished, not alongside.
- **Extensibility beyond the current plugin contract.** The TOML plugin system
  landed early — see `res/plugins/README.md`. What it deliberately does *not*
  do is let a plugin override a built-in previewer, which is what keeps
  installing one from being able to regress anything. If a real need appears
  for `.csv` as a table or `.json` through a pretty-printer, that is an
  additive `override = true` on a rule, and it is a decision worth making
  against a real case rather than in advance.

## Non-goals

Named so they stay decided:

- **Editing.** No markup, no trim, no rotate-and-save. A previewer that writes
  is an editor with worse affordances.
- **Extraction.** Archives are listed, never unpacked. Already policy; still
  policy.
- **A browser engine.** No WebKit, no CEF, no servo — for HTML, for Markdown,
  for anything.
- **Windows/macOS ports.** The layer-shell overlay *is* the product; a
  portable rewrite would be a different program.

## The standing rules

Every milestone, every previewer, the same contract — these are not steps on
the roadmap because they apply to all of them:

- Detection stays content-first; the extension is evidence only where the
  README says it is.
- `preview::load` keeps no error type. New failure modes fail into the card.
- The engine reports facts; `view.rs` chooses words. New strings go through
  Fluent from the first commit.
- Image handles are built when data lands, never in `view`.
- Anything asynchronous is generation-stamped.
- Wayland-native throughout: layer-shell, `ext-background-effect-v1`, and the
  color-management protocol when it arrives. No X11 code paths.
