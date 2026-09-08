// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Application state and the update loop.
//!
//! `peek` is a daemon, for the same reason `light` is: the expensive parts of
//! showing a preview — creating a wgpu device, loading syntect's syntax set,
//! building GStreamer's registry — happen once at startup rather than once per
//! keypress. A second invocation reaches the running instance over D-Bus and
//! only has to map a surface and decode one file.
//!
//! ## Four ways in
//!
//! * `peek <paths…>` on the command line, which becomes a D-Bus activation when
//!   the daemon is already up.
//! * `org.gnome.NautilusPreviewer2`, which is the space bar in Nautilus. See
//!   [`crate::previewer`].
//! * "Open With → Peek" from any file manager, including cosmic-files, via the
//!   desktop entry.
//! * `peek` with no arguments, which toggles whatever was last shown.
//!
//! All four converge on [`Message::Show`], so there is one code path that
//! decides what to preview.
//!
//! ## Stale loads
//!
//! Decoding runs on a blocking thread and takes anywhere from a millisecond to
//! most of a second. A user holding down the right arrow will start several
//! loads before the first finishes, and they will not finish in order. Every
//! load carries the generation it was started at, and a result whose generation
//! is not the current one is dropped — the same rule `light` applies to search
//! responses, for the same reason.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cosmic::Application as _;
use cosmic::app::{Core, Task};
use cosmic::iced::keyboard::Key;
use cosmic::iced::keyboard::key::Named;
use cosmic::iced::{Point, Size, Subscription, Vector, event, keyboard, window};
use cosmic::widget::image;
use peek_engine::{Entry, Neighbourhood, Options, Player, Preview};

use crate::anim::Panel;
use crate::config::Config;
use crate::previewer::{self, Direction};
use crate::surface;
use crate::view;

/// Reverse-DNS identifier, used for the D-Bus name and the config store.
pub const APP_ID: &str = "com.magnetaros.Peek";

/// D-Bus action name a second invocation uses to hand its paths over.
pub const SHOW_ACTION: &str = "show";

/// How far a seek key moves through a media file.
const SEEK_STEP: Duration = Duration::from_secs(5);

/// Longest edge, in logical pixels, an index-sheet thumbnail is rendered at.
///
/// Physical size is this times the scale factor, resolved when the load is
/// scheduled — a cell rendered at logical size is half resolution on a 2×
/// display, the same trap the vector and page targets avoid.
const THUMBNAIL_EDGE: u32 = 192;

/// Files the index sheet will render thumbnails for.
///
/// A decoded thumbnail is a quarter of a megabyte, and a home directory can
/// hold thousands of files. Past this the grid still lists every file — it
/// shows their type icons instead, which is what an undecoded cell falls back
/// to anyway.
const MAX_THUMBNAILS: usize = 250;

/// Frame callbacks requested after a seek, so the new frame is actually drawn.
///
/// A count rather than a deadline because it is the *frames* that matter: the
/// pipeline needs a handful of draw cycles to decode and deliver, and on a
/// 60 Hz display this is a fifth of a second.
const FRAMES_AFTER_SEEK: u8 = 12;

/// Startup flags.
///
/// `run_single_instance` forwards these to the running daemon: with no paths it
/// sends a plain activation, which is the toggle gesture, and with paths it
/// sends [`SHOW_ACTION`] carrying them.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    action: Option<String>,
    paths: Vec<String>,
}

impl Flags {
    /// Build flags from already-resolved absolute paths.
    #[must_use]
    pub fn new(paths: Vec<String>) -> Self {
        Self {
            // `None` means "activate", which the daemon reads as a toggle. The
            // distinction has to be made here rather than in the daemon,
            // because libcosmic picks the D-Bus method from it.
            action: (!paths.is_empty()).then(|| SHOW_ACTION.to_owned()),
            paths,
        }
    }

    /// Paths as owned values, for the instance that becomes the daemon.
    #[must_use]
    pub fn into_paths(self) -> Vec<PathBuf> {
        self.paths.into_iter().map(PathBuf::from).collect()
    }
}

impl cosmic::app::CosmicFlags for Flags {
    type SubCommand = String;
    type Args = Vec<String>;

    fn action(&self) -> Option<&Self::SubCommand> {
        self.action.as_ref()
    }

    fn args(&self) -> Vec<&str> {
        self.paths.iter().map(String::as_str).collect()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Preview these files, starting at `index`.
    Show { paths: Vec<PathBuf>, index: usize },
    /// A decode finished. Dropped unless `generation` is still current.
    Loaded {
        generation: u64,
        preview: Box<Preview>,
    },
    /// A playback pipeline came up for the current file.
    PlayerReady {
        generation: u64,
        player: std::sync::Arc<std::sync::Mutex<Option<Player>>>,
    },
    /// A neighbour finished decoding ahead of need.
    Preloaded {
        key: PreloadKey,
        path: PathBuf,
        preview: Box<Preview>,
    },
    /// Move through the neighbourhood by a signed offset.
    Step(isize),
    /// Turn the page of a multi-page document.
    Page(isize),
    /// Scale a visual preview by a multiplier.
    Zoom(f32),
    /// Return to the natural fit.
    ResetZoom,
    /// Jump between the fitted view and the file's own pixels — double-click.
    ToggleActualSize,
    /// The pointer moved over the preview content.
    PointerMoved(Point),
    /// A press began over the preview content: the start of a pan drag.
    DragStart,
    /// The press ended, or the pointer left the content mid-drag.
    DragEnd,
    /// A text preview was scrolled to this absolute offset.
    TextScrolled(f32),
    /// Show the content against the whole output, or come back.
    ToggleFullscreen,
    /// Space or escape: leave fullscreen if in it, otherwise dismiss.
    Escape,
    /// Put the current file's path on the clipboard.
    CopyPath,
    /// Show or hide the settings panel.
    ToggleSettings,
    /// Show or hide the index sheet: every file in the selection, at once.
    ToggleGrid,
    /// An arrow key, addressed to whatever is currently on screen.
    Navigate(Nav),
    /// Enter: open the cell in the sheet, or the file in its application.
    Activate,
    /// Move the index sheet's cursor by a signed number of cells.
    GridMove(isize),
    /// Preview a cell directly, from a click.
    GridPick(usize),
    /// A thumbnail finished rendering for the index sheet.
    Thumbnail {
        path: PathBuf,
        raster: Box<Option<peek_engine::Raster>>,
    },
    /// Change one setting and persist it.
    Setting(Setting),
    /// Start or stop playback.
    TogglePlay,
    /// Move through a media file by a signed number of seek steps.
    Seek(i32),
    /// Jump to a fraction of the media's duration, from the scrub bar.
    SeekTo(f32),
    /// Hand the file to its default application and dismiss.
    OpenExternally,
    /// The file manager asked for something.
    Previewer(previewer::Request),
    /// The previewer service came up.
    PreviewerReady(std::sync::Arc<previewer::Service>),
    /// Settings changed on disk and were re-read.
    ConfigChanged(Box<Config>),
    /// Begin dismissing. The surface outlives this until the fade finishes.
    Dismiss,
    /// Show the previewer, or dismiss it if already open.
    Toggle,
    /// A frame callback while animating or playing.
    Frame(Instant),
    /// The overlay surface was configured at this size.
    Configured(Size),
    /// The scale factor of the output the surface landed on.
    Scaled(f32),
    /// The desktop theme changed: palette, or the frosted setting, or both.
    ThemeChanged,
    /// Nothing to do. Used to discard the result of infallible platform tasks.
    None,
}

pub struct App {
    core: Core,
    /// Present only while the overlay is mapped.
    surface: Option<window::Id>,
    /// Size of the overlay surface, i.e. the output. Drives panel placement.
    screen: Size,

    /// The file being previewed and the ones either side of it.
    around: Neighbourhood,
    /// Resolved metadata for the current file. `None` before the first request.
    entry: Option<Entry>,
    preview: Preview,
    /// Incremented on every navigation; stamped onto each load so that results
    /// arriving out of order can be discarded.
    generation: u64,

    /// Scale factor of the output the surface is on.
    ///
    /// Surface sizes are logical; textures are uploaded in physical pixels. A
    /// vector or a PDF page rasterised at the logical size is half the
    /// resolution it is composited at on a 2× display, which is exactly the
    /// softness rendering-rather-than-scaling exists to avoid.
    scale: f32,

    /// GPU handle for whatever image the current preview draws.
    ///
    /// Built once when the preview lands rather than in `view`: every
    /// `Handle::from_rgba` takes a fresh `Id`, so a handle constructed in `view`
    /// misses the renderer's texture cache and re-uploads the whole image every
    /// frame.
    image: Option<image::Handle>,

    /// Frames of an animated image, playing whenever one is on screen.
    animation: Option<Animator>,

    /// A held-open PDF, so page turns pay for rendering and nothing else.
    ///
    /// Keyed by path: the session belongs to one document, and stepping to
    /// another file drops it.
    pdf_session: Option<(PathBuf, peek_engine::pdf::Session)>,

    /// Neighbours decoded ahead of the arrow keys.
    ///
    /// Holding an arrow through a directory should land on a decoded preview,
    /// not start a decode. Entries are only trusted while the options they
    /// were decoded under still hold — any change that shapes a decode clears
    /// the map — and only the current file's two neighbours are kept, so the
    /// memory bound is two previews.
    preload: HashMap<PathBuf, Box<Preview>>,
    /// Preloads currently decoding, so holding a key does not start the same
    /// decode twice.
    preload_inflight: HashSet<PathBuf>,

    /// How far a cropped visual preview is panned from centre. Clamped so the
    /// image's edges never leave the viewport, and reset on every navigation.
    pan: Vector,
    /// Whether a pan drag is in progress.
    dragging: bool,
    /// Last pointer position over the content, for drag deltas.
    cursor: Point,
    /// Absolute scroll offset of the current text preview, which is what the
    /// renderer windows the document around.
    text_scroll: f32,

    /// Whether the settings panel is showing instead of the preview.
    settings: bool,

    /// Whether the index sheet is showing instead of one file.
    ///
    /// QuickLook's other half: arrows walk a selection one file at a time, and
    /// the index sheet shows all of it at once. Both are the same
    /// neighbourhood — the grid is a second way to look at it, not a second
    /// place the selection is kept.
    grid: bool,
    /// Which cell the index sheet's cursor is on.
    grid_index: usize,
    /// Rendered cells, keyed by path. `None` records a file that has no
    /// thumbnail, so it is not asked for again on every frame.
    thumbnails: HashMap<PathBuf, Option<image::Handle>>,
    /// Thumbnails currently rendering, so a redraw does not start them twice.
    thumbnails_inflight: HashSet<PathBuf>,

    /// Whether the content is shown against the whole output.
    ///
    /// QuickLook's other surface: the panel's chrome yields and the file gets
    /// the display. Not a second window — the layer surface already covers the
    /// output, so this only changes how much of it the content is given.
    fullscreen: bool,

    /// Live playback, when the current file is media and a pipeline came up.
    player: Option<Player>,
    /// Frames still to be pulled after an operation that changes what is on
    /// screen without playback running.
    ///
    /// Seeking while paused decodes a new frame, but nothing is asking for one:
    /// the frame subscription only runs while playing, so without this the
    /// panel keeps showing the frame from before the seek and the scrub bar
    /// looks broken. A short countdown covers the pipeline's own latency in
    /// producing it.
    pending_frames: u8,
    /// Page of a multi-page document, carried across loads so that stepping
    /// away and back does not silently reset it.
    page: usize,
    zoom: f32,

    config: Config,
    panel: Panel,
    /// Set while the close transition plays, so input is ignored on the way out.
    dismissing: bool,
    /// Whether the blur region has been sent since the surface last drew.
    ///
    /// The region is double-buffered surface-local state and only takes effect
    /// against a committed buffer, so sending it before anything has rendered
    /// is silently dropped.
    blur_settled: bool,
    /// A decode deferred until the surface has been configured.
    ///
    /// Decoding is not started at the same time as the surface is created. On a
    /// first preview the compositor is still negotiating a buffer and wgpu is
    /// still building its device, and starting a GStreamer pipeline into that
    /// window races them for the GPU: observed as a surface that is created,
    /// reported as mapped, and never drawn, leaving the panel at zero opacity
    /// with the application believing it is visible. Waiting for the configure
    /// costs nothing — there is nothing to draw the preview onto until then.
    deferred_load: bool,
    /// The surface has been requested but has not been configured yet.
    ///
    /// The open transition starts when this clears rather than when the surface
    /// is requested: creating a layer surface and getting the first frame
    /// callback takes long enough that a transition started at request time is
    /// most of the way through before anything is on screen.
    awaiting_first_configure: bool,

    /// The `org.gnome.NautilusPreviewer2` service, when it could be started.
    previewer: Option<std::sync::Arc<previewer::Service>>,
}

impl App {
    /// Options for the next load, resolved against the display and the theme.
    fn options(&self) -> Options {
        Options {
            // Read live rather than stored: the desktop can switch modes while
            // the daemon is resident, and a preview loaded after the switch has
            // to use the new palette.
            dark: cosmic::theme::is_dark(),
            // SVGs are rendered rather than scaled, so the target has to be the
            // size they will actually occupy — in *physical* pixels, which is
            // what the texture is uploaded and sampled at.
            vector_target: (self.screen.width.max(self.screen.height) * self.scale) as u32,
            // Same reasoning for a PDF page, but bounded by the panel rather
            // than by the display: a page is drawn into a reading-sized column,
            // not across the whole output, and rasterising the difference is
            // work that is scaled straight back down.
            page_target: (self.screen.width.max(self.screen.height)
                * self.config.fraction()
                * self.scale) as u32,
            page: self.page,
        }
    }

    /// Start decoding the current file.
    ///
    /// Bumps the generation, so any load already in flight is discarded when it
    /// lands. The overlay keeps showing the previous preview until this one
    /// arrives, which is what makes holding an arrow key look like a sequence
    /// rather than a strobe.
    fn reload(&mut self) -> Task<Message> {
        let Some(path) = self.around.current().map(PathBuf::from) else {
            return Task::none();
        };

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;

        let Some(entry) = Entry::load(&path) else {
            // The file went away between being selected and being opened, which
            // is common enough — a download finishing, a build directory being
            // cleaned — that it is a preview rather than an error.
            self.entry = None;
            self.preview = Preview::Card(Box::default());
            self.image = None;
            self.animation = None;
            return Task::none();
        };

        // The held document belongs to the previous file once the path moves.
        if self
            .pdf_session
            .as_ref()
            .is_some_and(|(held, _)| held != &path)
        {
            self.pdf_session = None;
        }

        // Playback of the previous file stops here rather than when the next one
        // loads: the pipeline holds the audio device, and letting the outgoing
        // track keep playing over the incoming preview is the worst of both.
        self.player = None;
        self.entry = Some(entry.clone());
        self.page = self.page.min(page_count(&self.preview).saturating_sub(1));

        let options = self.options();

        // A neighbour decoded ahead of time makes this navigation free. Only
        // page zero is ever preloaded, so any other page has to decode.
        if options.page == 0
            && let Some(preview) = self.preload.remove(&path)
        {
            return self.apply_loaded(preview);
        }

        Task::future(async move {
            // Decoding is CPU work; `spawn_blocking` keeps it off the executor
            // that is also driving the compositor connection.
            let preview =
                tokio::task::spawn_blocking(move || peek_engine::preview::load(&entry, options))
                    .await
                    .unwrap_or_else(|error| {
                        // A decoder panicking is a bug in a codec, not a reason
                        // to lose the preview: the card still describes the file.
                        tracing::error!(%error, "the decoder thread panicked");
                        Preview::Card(Box::default())
                    });

            cosmic::action::app(Message::Loaded {
                generation,
                preview: Box::new(preview),
            })
        })
    }

    /// Install a finished preview and everything that follows from it.
    ///
    /// One function whether the preview arrived from a background load or from
    /// the preload cache, so the two paths cannot drift.
    fn apply_loaded(&mut self, mut preview: Box<Preview>) -> Task<Message> {
        // Animated frames become GPU handles once, here. They are *taken* from
        // the preview rather than copied: keeping the raster frames in state as
        // well would double a budget that is deliberately large.
        self.animation = None;
        if let Preview::Picture(picture) = preview.as_mut()
            && let Some(animation) = picture.animation.take()
        {
            self.animation = Some(Animator::new(animation, Instant::now()));
        }

        self.image = match &self.animation {
            Some(animator) => Some(animator.handles[0].clone()),
            None => view::handle_for(&preview),
        };

        let media = match preview.as_ref() {
            Preview::Media(media) if media.decodable => Some(media.has_video),
            _ => None,
        };
        // A specimen has to be set in the font it describes, which means the
        // file's faces reach the renderer before the panel draws.
        let font = match preview.as_ref() {
            Preview::Font(specimen) => crate::fonts::register(specimen),
            _ => None,
        };
        self.preview = *preview;

        let player = match media {
            Some(has_video) => self.start_player(has_video),
            None => Task::none(),
        };

        Task::batch([
            player,
            font.unwrap_or_else(Task::none),
            self.refresh_blur(),
            self.schedule_preloads(),
        ])
    }

    /// The options half of a preload's identity.
    fn preload_key(&self) -> PreloadKey {
        let options = self.options();
        PreloadKey {
            dark: options.dark,
            vector_target: options.vector_target,
            page_target: options.page_target,
        }
    }

    /// The two files an arrow press can reach from here.
    fn neighbour_paths(&self) -> Vec<PathBuf> {
        let paths = self.around.paths();
        let count = paths.len();
        if count <= 1 {
            return Vec::new();
        }

        let (current, _) = self.around.position();
        let index = current - 1;

        let next = paths[(index + 1) % count].clone();
        let previous = paths[(index + count - 1) % count].clone();

        let mut wanted = vec![next];
        if !wanted.contains(&previous) {
            wanted.push(previous);
        }
        wanted.retain(|path| Some(path.as_path()) != self.around.current());
        wanted
    }

    /// Start decoding the current file's neighbours, ahead of the arrow keys.
    fn schedule_preloads(&mut self) -> Task<Message> {
        let wanted = self.neighbour_paths();

        // Anything cached beyond the pair is stale the moment the user moved.
        self.preload.retain(|path, _| wanted.contains(path));

        if wanted.is_empty() {
            return Task::none();
        }

        let key = self.preload_key();
        let mut tasks = Vec::new();

        for path in wanted {
            if self.preload.contains_key(&path) || self.preload_inflight.contains(&path) {
                continue;
            }
            let Some(entry) = Entry::load(&path) else {
                continue;
            };

            self.preload_inflight.insert(path.clone());
            let options = Options {
                page: 0,
                ..self.options()
            };

            tasks.push(Task::future(async move {
                let preview = tokio::task::spawn_blocking(move || {
                    peek_engine::preview::load(&entry, options)
                })
                .await
                .unwrap_or_else(|error| {
                    tracing::error!(%error, "the preload thread panicked");
                    Preview::Card(Box::default())
                });

                cosmic::action::app(Message::Preloaded {
                    key,
                    path,
                    preview: Box::new(preview),
                })
            }));
        }

        Task::batch(tasks)
    }

    /// Render the current page through the held document.
    ///
    /// The session is created on the first page turn and reused after it: one
    /// header parse per document, however many pages are walked. Anything that
    /// goes wrong falls back to the stateless loader, which also re-derives
    /// the precise reason for the card.
    fn render_page(&mut self) -> Task<Message> {
        let Some(path) = self.around.current().map(PathBuf::from) else {
            return Task::none();
        };
        if !matches!(self.preview, Preview::Pdf(_)) {
            // A comic's page turn is a seek into the zip's central directory
            // and one decode, which needs no held state to be fast.
            return self.reload();
        }

        if self
            .pdf_session
            .as_ref()
            .is_none_or(|(held, _)| held != &path)
        {
            self.pdf_session = Some((path.clone(), peek_engine::pdf::Session::open(&path)));
        }
        let session = self
            .pdf_session
            .as_ref()
            .map(|(_, session)| session.clone())
            .expect("the session was just installed");

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let options = self.options();
        let entry = self.entry.clone();

        Task::future(async move {
            let preview = tokio::task::spawn_blocking(move || {
                match session.render(options.page, options.page_target) {
                    Ok(pdf) => Preview::Pdf(Box::new(pdf)),
                    Err(error) => {
                        tracing::debug!(%error, "page render fell back to a full load");
                        match &entry {
                            Some(entry) => peek_engine::preview::load(entry, options),
                            None => Preview::Card(Box::default()),
                        }
                    }
                }
            })
            .await
            .unwrap_or_else(|error| {
                tracing::error!(%error, "the page render thread panicked");
                Preview::Card(Box::default())
            });

            cosmic::action::app(Message::Loaded {
                generation,
                preview: Box::new(preview),
            })
        })
    }

    /// How the overlay is being shown, for the geometry functions.
    fn layout(&self) -> surface::Layout {
        surface::Layout {
            zoom: self.zoom,
            fullscreen: self.fullscreen,
            // Both are lists rather than a file, and want the same column.
            settings: self.settings || self.grid,
        }
    }

    /// Start rendering the thumbnails the index sheet needs.
    ///
    /// Every cell at once rather than as they scroll into view: the grid is
    /// bounded to [`MAX_THUMBNAILS`], and a cell that fills in while the eye
    /// is already on it reads worse than one that was never going to.
    fn schedule_thumbnails(&mut self) -> Task<Message> {
        let edge = (THUMBNAIL_EDGE as f32 * self.scale) as u32;
        let wanted: Vec<PathBuf> = self
            .around
            .paths()
            .iter()
            .take(MAX_THUMBNAILS)
            .filter(|path| {
                !self.thumbnails.contains_key(*path) && !self.thumbnails_inflight.contains(*path)
            })
            .cloned()
            .collect();

        let mut tasks = Vec::with_capacity(wanted.len());
        for path in wanted {
            self.thumbnails_inflight.insert(path.clone());
            tasks.push(Task::future(async move {
                let rendered = {
                    let path = path.clone();
                    tokio::task::spawn_blocking(move || {
                        Entry::load(&path)
                            .and_then(|entry| peek_engine::thumbnail::render(&entry, edge))
                    })
                    .await
                    .unwrap_or_else(|error| {
                        tracing::error!(%error, "a thumbnail thread panicked");
                        None
                    })
                };

                cosmic::action::app(Message::Thumbnail {
                    path,
                    raster: Box::new(rendered),
                })
            }));
        }

        Task::batch(tasks)
    }

    /// Keep the pan inside the cropped area, or at centre when nothing crops.
    fn clamp_pan(&mut self) {
        let (max_x, max_y) =
            surface::max_pan(self.screen, &self.preview, &self.config, self.layout());
        self.pan.x = self.pan.x.clamp(-max_x, max_x);
        self.pan.y = self.pan.y.clamp(-max_y, max_y);
    }

    /// Build a playback pipeline for the current file.
    ///
    /// Separate from [`Self::reload`] because opening a `playbin` prerolls a
    /// decoder, and doing that for every file the user arrows past would leave a
    /// pipeline per file alive for as long as it takes them to let go of the key.
    fn start_player(&self, has_video: bool) -> Task<Message> {
        let Some(path) = self.around.current().map(PathBuf::from) else {
            return Task::none();
        };
        let generation = self.generation;

        Task::future(async move {
            let player = tokio::task::spawn_blocking(move || Player::open(&path, has_video).ok())
                .await
                .unwrap_or_default();

            // Wrapped rather than sent directly: `Player` owns a GStreamer
            // pipeline and is not `Clone`, but a libcosmic message must be.
            cosmic::action::app(Message::PlayerReady {
                generation,
                player: std::sync::Arc::new(std::sync::Mutex::new(player)),
            })
        })
    }

    /// Re-send the blur region so it matches the panel's current geometry.
    fn refresh_blur(&self) -> Task<Message> {
        let Some(id) = self.surface else {
            return Task::none();
        };
        let rect = surface::panel_rect(self.screen, &self.preview, &self.config, self.layout());
        surface::set_blur(id, rect, self.blur_wanted()).map(|()| cosmic::action::app(Message::None))
    }

    /// Whether the backdrop behind the panel should be blurred.
    ///
    /// The desktop's own answer first. COSMIC carries the user's decision about
    /// frosted effects in the theme, and `frosted_system_interface` is the field
    /// that governs system UI — the layer above the desktop that the launcher
    /// and this overlay both live on. Peek's own key is an opt-out from there,
    /// not a replacement for it, so turning frosting off system-wide turns it
    /// off here too.
    fn blur_wanted(&self) -> bool {
        self.config.blur && cosmic::theme::active().cosmic().frosted_system_interface
    }

    /// Show the overlay.
    fn open(&mut self) -> Task<Message> {
        if let Some(previewer) = &self.previewer {
            previewer.set_visible(true);
        }

        if self.surface.is_some() {
            // Already mapped; make sure the transition is running forward.
            self.dismissing = false;
            self.panel.open(Instant::now());
            return Task::none();
        }

        let id = window::Id::unique();
        self.surface = Some(id);
        self.dismissing = false;
        // Deliberately not started here — see `awaiting_first_configure`.
        self.awaiting_first_configure = true;

        surface::open(id).map(|()| cosmic::action::app(Message::None))
    }

    /// Begin hiding the overlay. The surface is destroyed later, once the fade
    /// has played out.
    fn dismiss(&mut self) -> Task<Message> {
        if self.surface.is_none() || self.dismissing {
            return Task::none();
        }

        self.settings = false;

        // Leaving fullscreen is what the next open should start from: a
        // previewer that reopens filling the display because of a mode set
        // three files ago is one that has forgotten what the user last did
        // deliberately.
        self.fullscreen = false;

        self.dismissing = true;
        self.blur_settled = false;
        self.panel.close(Instant::now());

        // Playback stops now rather than when the surface goes: the fade takes
        // 120 ms, and audio continuing through it sounds like a bug.
        self.player = None;

        if let Some(previewer) = &self.previewer {
            previewer.set_visible(false);
        }

        Task::none()
    }

    /// Destroy the surface once the close transition has played out.
    fn finish_close(&mut self) -> Task<Message> {
        let Some(id) = self.surface.take() else {
            return Task::none();
        };
        self.dismissing = false;
        // The preview is *not* cleared. Reopening with no arguments shows the
        // last file again, which is what makes `peek` on its own a toggle rather
        // than a way to open an empty window. The preload cache *is* cleared:
        // two decoded neighbours are real memory, and a hidden previewer has no
        // claim to it.
        self.preload.clear();
        // Thumbnails are the larger cache of the two and are only wanted by a
        // grid that is no longer on screen.
        self.thumbnails.clear();
        surface::close(id).map(|()| cosmic::action::app(Message::None))
    }

    /// Move through the neighbourhood.
    fn step(&mut self, delta: isize, direction: Direction) -> Task<Message> {
        if !self.around.step(delta) {
            return Task::none();
        }

        // Zoom, pan, and scroll are per-file: carrying a 3× zoom onto the next
        // photo means the next photo opens cropped, which is never what was
        // wanted.
        self.zoom = 1.0;
        self.pan = Vector::new(0.0, 0.0);
        self.dragging = false;
        self.text_scroll = 0.0;
        self.page = 0;
        self.panel.stepped(Instant::now(), delta > 0);

        // Tell the file manager to follow, so closing the preview leaves the
        // cursor on the file that was last shown.
        if self.config.follow_selection
            && let Some(previewer) = &self.previewer
        {
            previewer.move_selection(direction);
        }

        Task::batch([self.reload(), self.refresh_blur()])
    }

    /// Hand the current file to its default application.
    fn open_externally(&mut self) -> Task<Message> {
        let Some(entry) = &self.entry else {
            return Task::none();
        };
        let Some(uri) = previewer::uri_from_path(&entry.path) else {
            return Task::none();
        };

        // Spawned detached, in its own systemd scope, so the opened application
        // outlives the previewer rather than dying with it.
        Task::batch([open_uri(&uri), self.dismiss()])
    }

    /// Handle a request from the file manager.
    fn previewer_request(&mut self, request: previewer::Request) -> Task<Message> {
        match request {
            previewer::Request::Show {
                path,
                close_if_shown,
            } => {
                // The toggle half of the gesture: pressing space twice on the
                // same file closes the preview rather than reloading it.
                let same = self.around.current() == Some(path.as_path());
                if close_if_shown && same && self.surface.is_some() && !self.dismissing {
                    return self.dismiss();
                }

                self.update(Message::Show {
                    paths: vec![path],
                    index: 0,
                })
            }
            previewer::Request::Close => self.dismiss(),
        }
    }
}

/// The options a preload was decoded under, reduced to what shapes the result.
///
/// Compared rather than trusted: the theme can flip or the surface can move
/// to another display while a preload is in flight, and a preview decoded for
/// the old world must not be cached into the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreloadKey {
    dark: bool,
    vector_target: u32,
    page_target: u32,
}

/// Plays a decoded animation by swapping pre-built GPU handles.
///
/// The handles are built once, when the preview lands — the same rule every
/// other image in the overlay follows — so advancing a frame is a clone of an
/// `Arc`, not an upload.
struct Animator {
    handles: Vec<image::Handle>,
    delays: Vec<Duration>,
    index: usize,
    /// When the current frame went up, advanced by each frame's own delay so
    /// timing does not drift with the display rate.
    shown_at: Instant,
}

impl Animator {
    fn new(animation: peek_engine::picture::Animation, now: Instant) -> Self {
        let mut handles = Vec::with_capacity(animation.frames.len());
        let mut delays = Vec::with_capacity(animation.frames.len());
        for frame in animation.frames {
            delays.push(frame.delay);
            // By value: the frames were decoded for these handles and nothing
            // else holds them.
            handles.push(view::handle_from_raster(frame.raster));
        }
        Self {
            handles,
            delays,
            index: 0,
            shown_at: now,
        }
    }

    /// Move to the frame that should be showing at `now`.
    ///
    /// Returns the new frame's handle when it changed, `None` when the current
    /// frame is still the right one. Catches up over a short stall and
    /// re-bases over a long one, so a preview that was hidden for a minute
    /// resumes rather than replaying the minute.
    fn advance(&mut self, now: Instant) -> Option<image::Handle> {
        let mut advanced = false;

        for _ in 0..8 {
            let delay = self.delays[self.index];
            if now.saturating_duration_since(self.shown_at) < delay {
                break;
            }
            self.shown_at += delay;
            self.index = (self.index + 1) % self.handles.len();
            advanced = true;
        }

        if now.saturating_duration_since(self.shown_at) > Duration::from_secs(1) {
            self.shown_at = now;
        }

        advanced.then(|| self.handles[self.index].clone())
    }
}

/// Which way an arrow key pointed.
///
/// The key is turned into a direction rather than an action, because what a
/// direction *means* depends on what is on screen — a cell in the index
/// sheet, a page in a document, or the next file — and that decision belongs
/// with the state, not with the key handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Left,
    Right,
    Up,
    Down,
}

/// One change the settings panel can make.
///
/// An enum rather than a closure per control so that [`Setting::apply`] and
/// [`Setting::affects_decoding`] sit together: whether a setting invalidates
/// decoded previews is a property of the setting, and keeping it beside the
/// mutation is what stops the two drifting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Setting {
    Blur(bool),
    Opacity(f32),
    Autoplay(crate::config::Autoplay),
    LoopMedia(bool),
    LineNumbers(bool),
    MaxFraction(f32),
    FollowSelection(bool),
    ClickAway(bool),
    Animate(bool),
}

impl Setting {
    fn apply(self, config: &mut Config) {
        match self {
            Self::Blur(value) => config.blur = value,
            Self::Opacity(value) => config.opacity = value,
            Self::Autoplay(value) => config.autoplay = value,
            Self::LoopMedia(value) => config.loop_media = value,
            Self::LineNumbers(value) => config.line_numbers = value,
            Self::MaxFraction(value) => config.max_fraction = value,
            Self::FollowSelection(value) => config.follow_selection = value,
            Self::ClickAway(value) => config.click_away = value,
            Self::Animate(value) => config.animate = value,
        }
    }

    /// Whether changing this invalidates an already-decoded preview.
    ///
    /// Only the panel's share of the display does: it is what the vector and
    /// page render targets are derived from. Everything else changes how the
    /// preview is *drawn*, which the next frame handles on its own.
    fn affects_decoding(self) -> bool {
        matches!(self, Self::MaxFraction(_))
    }
}

/// Number of pages a preview has, for bounding page navigation.
fn page_count(preview: &Preview) -> usize {
    match preview {
        Preview::Pdf(pdf) => pdf.pages,
        Preview::Comic(comic) => comic.pages,
        _ => 1,
    }
}

/// Hand a URI to the desktop's handler for it.
///
/// `xdg-open` rather than resolving the handler ourselves: it consults the same
/// mimeapps configuration the file manager does, so "Open" from the preview
/// lands in the application the user has already chosen for that type.
///
/// Spawned through libcosmic rather than through `std::process`, for two
/// reasons that both matter to a process that stays resident. It double-forks
/// and reaps the intermediate child, so a session's worth of "Open" presses
/// does not leave a session's worth of zombies behind; and it registers the
/// handler's PID as its own systemd scope, so the opened application belongs to
/// the session rather than to the previewer and survives the previewer exiting.
fn open_uri(uri: &str) -> Task<Message> {
    // Quoted: a path can contain spaces, and the exec string is split with
    // shell rules on the other side.
    let exec = format!("xdg-open {}", shell_quote(uri));
    Task::future(async move {
        cosmic::desktop::spawn_desktop_exec(
            exec,
            std::iter::empty::<(&str, &str)>(),
            Some(APP_ID),
            // Not a terminal program: `xdg-open` returns immediately and the
            // handler it starts draws its own window.
            false,
        )
        .await;
        cosmic::action::app(Message::None)
    })
}

/// Wrap a string in single quotes for a shell-style exec line.
///
/// The exec string is parsed with `shlex`, so a bare path containing a space
/// would arrive as two arguments and open nothing.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

impl cosmic::Application for App {
    type Executor = cosmic::executor::Default;
    type Flags = Flags;
    type Message = Message;

    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(mut core: Core, flags: Self::Flags) -> (Self, Task<Self::Message>) {
        // System UI, not an application window. libcosmic switches its frosted
        // and corner-radius decisions on this, and a previewer that maps a
        // layer surface over the whole output is in the same class as the
        // launcher, not in the class of a window with a title bar.
        core.set_app_type(cosmic::core::AppType::System);

        // The overlay owns the keyboard outright and handles every key itself.
        // libcosmic's navigation subscription would otherwise interpret the
        // same presses in parallel — Tab and Shift+Tab as focus traversal,
        // Escape as a second dismissal, F11 as a maximise for a window that
        // does not exist.
        core.set_keyboard_nav(false);

        let paths = flags.into_paths();

        let mut app = Self {
            core,
            surface: None,
            screen: Size::new(1920.0, 1080.0),
            scale: 1.0,
            around: Neighbourhood::default(),
            entry: None,
            preview: Preview::Pending,
            generation: 0,
            image: None,
            animation: None,
            pdf_session: None,
            preload: HashMap::new(),
            preload_inflight: HashSet::new(),
            pan: Vector::new(0.0, 0.0),
            dragging: false,
            cursor: Point::ORIGIN,
            text_scroll: 0.0,
            settings: false,
            grid: false,
            grid_index: 0,
            thumbnails: HashMap::new(),
            thumbnails_inflight: HashSet::new(),
            fullscreen: false,
            player: None,
            pending_frames: 0,
            page: 0,
            zoom: 1.0,
            config: Config::load(),
            panel: Panel::new(),
            dismissing: false,
            blur_settled: false,
            deferred_load: false,
            awaiting_first_configure: false,
            previewer: None,
        };

        app.panel.set_animated(app.config.animate);

        // Started with files: show them. Started without: stay resident and wait
        // for the file manager, which is how the service ends up running before
        // the first space is pressed.
        let initial = if paths.is_empty() {
            Task::none()
        } else {
            app.update(Message::Show { paths, index: 0 })
        };

        (app, initial)
    }

    #[allow(clippy::too_many_lines)]
    fn update(&mut self, message: Self::Message) -> Task<Self::Message> {
        match message {
            Message::None => Task::none(),

            Message::Show { paths, index } => {
                self.around = match paths.len() {
                    0 => return Task::none(),
                    // One file names a *position*, not a set: the rest of the
                    // directory is what the arrow keys move through.
                    1 => Neighbourhood::around(&paths[0]),
                    // Several files name the set outright, so the directory is
                    // not consulted.
                    _ => Neighbourhood::from_list(paths, index),
                };

                self.zoom = 1.0;
                self.pan = Vector::new(0.0, 0.0);
                self.dragging = false;
                self.text_scroll = 0.0;
                self.page = 0;
                self.grid = false;
                self.grid_index = 0;
                // A new selection is a new set of cells; the old ones describe
                // files that may not be in it.
                self.thumbnails.clear();
                self.panel.stepped(Instant::now(), true);

                let open = self.open();
                if self.awaiting_first_configure {
                    // Surface still being negotiated — see `deferred_load`.
                    self.deferred_load = true;
                    return open;
                }

                Task::batch([open, self.reload()])
            }

            Message::Loaded {
                generation,
                preview,
            } => {
                // Stale-response guard. A load that started before the last
                // navigation describes a file that is no longer on screen, and
                // showing it would make holding an arrow key land on the wrong
                // file.
                if generation != self.generation {
                    return Task::none();
                }

                self.apply_loaded(preview)
            }

            Message::Preloaded { key, path, preview } => {
                self.preload_inflight.remove(&path);
                // Kept only while everything that shaped the decode still
                // holds: the options, the file's place beside the cursor, and
                // an overlay that is actually up to spend the memory on.
                if self.surface.is_some()
                    && key == self.preload_key()
                    && self.neighbour_paths().contains(&path)
                {
                    self.preload.insert(path, preview);
                }
                Task::none()
            }

            Message::PlayerReady { generation, player } => {
                if generation != self.generation {
                    // The user moved on while the pipeline was starting. The
                    // `Arc` is dropped here, which tears the pipeline down.
                    return Task::none();
                }

                let player = player.lock().ok().and_then(|mut guard| guard.take());
                let Some(player) = player else {
                    return Task::none();
                };

                let has_video = matches!(&self.preview, Preview::Media(media) if media.has_video);
                if self.config.autoplay.applies(has_video) {
                    player.play();
                }
                self.player = Some(player);
                // A state change is asynchronous, so the subscription has to be
                // kept alive across it: without this the loop is re-evaluated
                // once, while the pipeline is still transitioning, and then
                // never again because nothing else produces a message.
                self.pending_frames = FRAMES_AFTER_SEEK;
                Task::none()
            }

            Message::Step(delta) => {
                let direction = if delta > 0 {
                    Direction::Right
                } else {
                    Direction::Left
                };
                self.step(delta, direction)
            }

            Message::Page(delta) => {
                let pages = page_count(&self.preview);
                if pages <= 1 {
                    // Nothing to page through, so the vertical arrows move
                    // between files instead. That is not a fallback for its own
                    // sake: a file manager in list view lays its selection out
                    // vertically, and the direction reported below is what makes
                    // its cursor follow the same way it does in grid view.
                    let direction = if delta > 0 {
                        Direction::Down
                    } else {
                        Direction::Up
                    };
                    return self.step(delta, direction);
                }

                // Clamped rather than wrapped, unlike stepping between files:
                // a document has a first and a last page, and jumping from the
                // end back to the cover is disorienting in a way that moving
                // from the last photo to the first is not.
                let page = (self.page as isize + delta).clamp(0, pages as isize - 1) as usize;
                if page == self.page {
                    return Task::none();
                }

                self.page = page;
                self.panel.stepped(Instant::now(), delta > 0);
                self.render_page()
            }

            Message::Zoom(factor) => {
                self.zoom = (self.zoom * factor).clamp(surface::MIN_ZOOM, surface::MAX_ZOOM);
                self.clamp_pan();
                self.refresh_blur()
            }

            Message::ResetZoom => {
                self.zoom = 1.0;
                self.pan = Vector::new(0.0, 0.0);
                self.refresh_blur()
            }

            Message::ToggleActualSize => {
                // 100% means the file's own pixels: the zoom at which one
                // texel is one physical pixel of the fitted view. For content
                // without a fixed resolution, a plain magnification stands in.
                let target = match (
                    surface::fit_size(self.screen, &self.preview, &self.config, self.layout()),
                    self.preview.natural_size(),
                ) {
                    (Some(fit), Some((width, _))) if fit.width > 0.0 => {
                        (width as f32 / fit.width).max(1.0)
                    }
                    _ => 2.0,
                };

                self.zoom = if (self.zoom - 1.0).abs() > 0.01 {
                    1.0
                } else {
                    target.clamp(surface::MIN_ZOOM, surface::MAX_ZOOM)
                };
                self.pan = Vector::new(0.0, 0.0);
                self.refresh_blur()
            }

            Message::PointerMoved(point) => {
                if self.dragging {
                    self.pan += point - self.cursor;
                    self.clamp_pan();
                }
                self.cursor = point;
                Task::none()
            }

            Message::DragStart => {
                self.dragging = true;
                Task::none()
            }

            Message::DragEnd => {
                self.dragging = false;
                Task::none()
            }

            Message::TextScrolled(offset) => {
                self.text_scroll = offset.max(0.0);
                Task::none()
            }

            Message::ToggleFullscreen => {
                self.fullscreen = !self.fullscreen;
                // The panel's geometry is what the blur region traces, and
                // fullscreen changes it completely.
                self.pan = Vector::new(0.0, 0.0);
                self.refresh_blur()
            }

            Message::ToggleGrid => {
                self.grid = !self.grid;
                if !self.grid {
                    return self.refresh_blur();
                }

                // The cursor starts on the file that was on screen, so leaving
                // the grid without moving lands back where it began.
                self.grid_index = self.around.position().0.saturating_sub(1);
                self.settings = false;
                Task::batch([self.schedule_thumbnails(), self.refresh_blur()])
            }

            Message::GridMove(delta) => {
                let count = self.around.len();
                if count == 0 {
                    return Task::none();
                }
                // Clamped rather than wrapped: a grid has edges the user can
                // see, and jumping from the last cell to the first when they
                // press right at the end reads as a fault.
                self.grid_index =
                    (self.grid_index as isize + delta).clamp(0, count as isize - 1) as usize;
                Task::none()
            }

            Message::Navigate(direction) => {
                if self.grid {
                    // Derived from the same function the sheet lays out with,
                    // so a row here is a row there.
                    let panel = surface::panel_rect(
                        self.screen,
                        &self.preview,
                        &self.config,
                        self.layout(),
                    );
                    let columns = surface::grid_columns(panel.width) as isize;
                    let delta = match direction {
                        Nav::Left => -1,
                        Nav::Right => 1,
                        Nav::Up => -columns,
                        Nav::Down => columns,
                    };
                    return self.update(Message::GridMove(delta));
                }

                match direction {
                    Nav::Left => self.update(Message::Step(-1)),
                    Nav::Right => self.update(Message::Step(1)),
                    Nav::Up => self.update(Message::Page(-1)),
                    Nav::Down => self.update(Message::Page(1)),
                }
            }

            Message::Activate => {
                if self.grid {
                    return self.update(Message::GridPick(self.grid_index));
                }
                self.open_externally()
            }

            Message::GridPick(index) => {
                let Some(path) = self.around.paths().get(index).cloned() else {
                    return Task::none();
                };
                self.grid = false;
                self.around.focus(&path);
                self.zoom = 1.0;
                self.pan = Vector::new(0.0, 0.0);
                self.page = 0;
                self.text_scroll = 0.0;
                self.panel.stepped(Instant::now(), true);
                Task::batch([self.reload(), self.refresh_blur()])
            }

            Message::Thumbnail { path, raster } => {
                self.thumbnails_inflight.remove(&path);
                self.thumbnails
                    .insert(path, raster.map(view::handle_from_raster));
                Task::none()
            }

            Message::ToggleSettings => {
                self.settings = !self.settings;
                self.grid = false;
                // The panel is sized around what it shows, and the settings
                // list is not the file.
                self.refresh_blur()
            }

            Message::Setting(change) => {
                change.apply(&mut self.config);
                self.config.store();
                self.panel.set_animated(self.config.animate);

                // A setting that shapes a decode invalidates what was decoded
                // under the old one — the same rule the config subscription
                // applies when the change arrives from outside.
                let redecode = change.affects_decoding();
                self.preload.clear();

                if redecode {
                    Task::batch([self.reload(), self.refresh_blur()])
                } else {
                    self.refresh_blur()
                }
            }

            Message::CopyPath => {
                let Some(entry) = &self.entry else {
                    return Task::none();
                };
                cosmic::iced::clipboard::write(entry.path.display().to_string())
            }

            Message::TogglePlay => {
                if let Some(player) = &self.player {
                    if player.is_playing() {
                        player.pause();
                    } else {
                        player.play();
                    }
                    // Same reason as `PlayerReady`: the pipeline has not
                    // finished changing state yet, and the transport has to
                    // redraw once it has.
                    self.pending_frames = FRAMES_AFTER_SEEK;
                }
                Task::none()
            }

            Message::Seek(steps) => {
                if let Some(player) = &self.player {
                    let current = player.position().unwrap_or_default();
                    let offset = SEEK_STEP * steps.unsigned_abs();
                    let target = if steps.is_negative() {
                        current.saturating_sub(offset)
                    } else {
                        current + offset
                    };
                    player.seek(target.min(player.duration().unwrap_or(target)));
                    self.pending_frames = FRAMES_AFTER_SEEK;
                }
                Task::none()
            }

            Message::SeekTo(fraction) => {
                if let Some(player) = &self.player
                    && let Some(duration) = player.duration()
                {
                    player.seek(duration.mul_f32(fraction.clamp(0.0, 1.0)));
                    self.pending_frames = FRAMES_AFTER_SEEK;
                }
                Task::none()
            }

            Message::OpenExternally => self.open_externally(),

            Message::Previewer(request) => self.previewer_request(request),

            Message::PreviewerReady(service) => {
                tracing::debug!(mapped = self.surface.is_some(), "previewer service ready");
                // The service may come up after a preview is already on screen —
                // `peek photo.png` shows the file before the bus connection is
                // negotiated — so the current visibility has to be pushed rather
                // than assumed false.
                service.set_visible(self.surface.is_some() && !self.dismissing);
                self.previewer = Some(service);
                Task::none()
            }

            Message::ConfigChanged(config) => {
                let blur_changed = config.blur != self.config.blur;
                self.config = *config;
                self.panel.set_animated(self.config.animate);
                // The panel fraction shapes render targets, so cached decodes
                // may describe the old settings.
                self.preload.clear();
                tracing::info!(blur_changed, "settings updated");
                self.refresh_blur()
            }

            Message::ThemeChanged => {
                // The palette decides how source is highlighted, and the
                // frosted setting decides whether the backdrop is blurred, so a
                // theme change has to reach both. Preloads were highlighted for
                // the old palette.
                self.preload.clear();
                Task::batch([self.reload(), self.refresh_blur()])
            }

            Message::Dismiss => self.dismiss(),

            Message::Escape => {
                // One key that always means "back": out of fullscreen first,
                // and only then out of the preview. Dismissing straight from
                // fullscreen would lose the file *and* the mode in one press.
                if self.settings {
                    self.settings = false;
                    return self.refresh_blur();
                }
                if self.grid {
                    self.grid = false;
                    return self.refresh_blur();
                }
                if self.fullscreen {
                    self.fullscreen = false;
                    self.pan = Vector::new(0.0, 0.0);
                    return self.refresh_blur();
                }
                self.dismiss()
            }

            Message::Toggle => {
                if self.surface.is_some() && !self.dismissing {
                    self.dismiss()
                } else if self.around.is_empty() {
                    // Nothing has ever been previewed, so there is nothing to
                    // toggle back to. Opening an empty overlay would be worse
                    // than doing nothing.
                    tracing::info!("nothing to show; pass a file path");
                    Task::none()
                } else {
                    let open = self.open();
                    if self.awaiting_first_configure {
                        self.deferred_load = true;
                        return open;
                    }
                    Task::batch([open, self.reload()])
                }
            }

            Message::Scaled(scale) => {
                // Guard against a nonsensical report rather than dividing the
                // render targets down to nothing.
                let scale = if scale.is_finite() && scale > 0.0 {
                    scale
                } else {
                    1.0
                };

                if (scale - self.scale).abs() < f32::EPSILON {
                    return Task::none();
                }

                tracing::debug!(scale, "surface scale factor");
                self.scale = scale;
                // Preloads were rasterised against the old factor.
                self.preload.clear();

                // Rasterised-for-a-size previews were produced against the old
                // factor and are now the wrong resolution; everything else is
                // resolution-independent or already at its natural size.
                if matches!(self.preview, Preview::Picture(_) | Preview::Pdf(_)) {
                    return self.reload();
                }

                Task::none()
            }

            Message::Configured(size) => {
                let resized = size != self.screen;
                self.screen = size;
                self.blur_settled = false;
                if resized {
                    // The render targets follow the output's size.
                    self.preload.clear();
                }

                // Asked for on every configure rather than once: a layer
                // surface can be moved between outputs, and the factor that
                // matters is the one for the output it is on now.
                let scale = match self.surface {
                    Some(id) => window::scale_factor(id)
                        .map(|scale| cosmic::action::app(Message::Scaled(scale))),
                    None => Task::none(),
                };

                if self.awaiting_first_configure {
                    self.awaiting_first_configure = false;
                    // The surface exists and is about to draw, so the transition
                    // now has somewhere to play.
                    self.panel.open(Instant::now());
                }

                tracing::debug!(?size, deferred = self.deferred_load, "surface configured");

                if self.deferred_load {
                    self.deferred_load = false;
                    // The load starts before the scale factor is known. That is
                    // deliberate: showing the file is the whole interaction, and
                    // waiting a round trip to learn the factor would delay it.
                    // On a 1× display `Scaled` then reports no change and costs
                    // nothing; on a 2× one it re-renders the vector formats once.
                    return Task::batch([self.reload(), self.refresh_blur(), scale]);
                }

                // An SVG is rendered for a specific size, so a display change
                // means it has to be rendered again — this is the one preview
                // that is not resolution-independent once it has been produced.
                if resized && matches!(self.preview, Preview::Picture(_)) {
                    return Task::batch([self.reload(), self.refresh_blur(), scale]);
                }

                Task::batch([self.refresh_blur(), scale])
            }

            Message::Frame(now) => {
                if self.dismissing && self.panel.is_closed(now) {
                    return self.finish_close();
                }

                // A frame callback means a buffer has been committed, so the
                // blur region now has something to attach to.
                if !self.blur_settled {
                    self.blur_settled = true;
                    return self.refresh_blur();
                }

                self.pending_frames = self.pending_frames.saturating_sub(1);

                if let Some(animator) = &mut self.animation
                    && let Some(handle) = animator.advance(now)
                {
                    self.image = Some(handle);
                }

                if let Some(player) = &self.player {
                    if let Some(raster) = player.frame() {
                        // By value: the frame is decoded for this handle and
                        // nothing else holds it, so moving the buffer in saves a
                        // full-frame copy on every frame that is drawn.
                        self.image = Some(view::handle_from_raster(raster));
                    }
                    if player.is_finished() {
                        if self.config.loop_media {
                            player.seek(Duration::ZERO);
                            player.play();
                        } else {
                            player.pause();
                        }
                    }
                }

                Task::none()
            }
        }
    }

    fn view(&self) -> cosmic::Element<'_, Self::Message> {
        // The overlay has no main window, so this is only reached if one is
        // somehow created. Render nothing rather than panicking.
        cosmic::widget::text("").into()
    }

    fn view_window(&self, id: window::Id) -> cosmic::Element<'_, Self::Message> {
        if Some(id) != self.surface {
            return cosmic::widget::text("").into();
        }

        view::overlay(view::Frame {
            panel: &self.panel,
            now: Instant::now(),
            screen: self.screen,
            preview: &self.preview,
            entry: self.entry.as_ref(),
            image: self.image.as_ref(),
            around: &self.around,
            player: self.player.as_ref(),
            zoom: self.zoom,
            pan: self.pan,
            text_scroll: self.text_scroll,
            animated: self.animation.is_some(),
            fullscreen: self.fullscreen,
            settings: self.settings,
            grid: self.grid,
            grid_index: self.grid_index,
            thumbnails: &self.thumbnails,
            config: &self.config,
        })
    }

    fn subscription(&self) -> Subscription<Self::Message> {
        let mut subscriptions = vec![
            // Owns the session-bus connection that serves the file manager.
            Subscription::run(previewer_stream),
            // Settings changes, delivered by cosmic-config without a restart.
            cosmic::cosmic_config::config_subscription::<_, Config>(
                std::any::TypeId::of::<ConfigSubscription>(),
                APP_ID.into(),
                <Config as cosmic::cosmic_config::CosmicConfigEntry>::VERSION,
            )
            .map(|update| {
                for error in update.errors {
                    tracing::warn!(%error, "ignoring an unreadable setting");
                }
                Message::ConfigChanged(Box::new(update.config))
            }),
            event::listen_with(keys),
        ];

        // Frame callbacks only while something is moving. Playback counts:
        // pulling a decoded frame has to happen at display rate, but only while
        // there is a decoder producing them.
        let playing = self.player.as_ref().is_some_and(Player::is_playing);
        if self.surface.is_some()
            && (self.awaiting_first_configure
                || playing
                // An animated image plays for as long as it is on screen.
                || self.animation.is_some()
                || self.pending_frames > 0
                || self.panel.is_animating(Instant::now()))
        {
            subscriptions.push(window::frames().map(|(_, at)| Message::Frame(at)));
        }

        Subscription::batch(subscriptions)
    }

    fn on_escape(&mut self) -> Task<Self::Message> {
        self.dismiss()
    }

    /// The desktop's palette changed under a resident daemon.
    ///
    /// Both hooks matter and they are not the same event: the mode hook fires
    /// when the user switches between light and dark, and the theme hook fires
    /// when the colours or the frosted settings of the current mode are edited.
    /// A previewer that stays running for the whole session sees both.
    fn system_theme_update(
        &mut self,
        _keys: &[&'static str],
        _theme: &cosmic::cosmic_theme::Theme,
    ) -> Task<Self::Message> {
        self.update(Message::ThemeChanged)
    }

    fn system_theme_mode_update(
        &mut self,
        _keys: &[&'static str],
        _mode: &cosmic::cosmic_theme::ThemeMode,
    ) -> Task<Self::Message> {
        self.update(Message::ThemeChanged)
    }

    fn dbus_activation(
        &mut self,
        message: cosmic::dbus_activation::Message,
    ) -> Task<Self::Message> {
        use cosmic::dbus_activation::Details;

        match message.msg {
            // `peek` with no arguments while the daemon is up: the toggle
            // gesture, so a keybinding can be the command itself.
            Details::Activate => self.update(Message::Toggle),

            // `peek <paths…>` while the daemon is up.
            Details::ActivateAction { action, args } if action == SHOW_ACTION => {
                self.update(Message::Show {
                    paths: args.into_iter().map(PathBuf::from).collect(),
                    index: 0,
                })
            }

            // A desktop entry launched us with files — "Open With → Peek".
            Details::Open { url } => self.update(Message::Show {
                paths: url
                    .iter()
                    .filter_map(|url| url.to_file_path().ok())
                    .collect(),
                index: 0,
            }),

            Details::ActivateAction { action, .. } => {
                tracing::warn!(action, "ignoring an unknown activation");
                Task::none()
            }
        }
    }
}

/// Type tag identifying the settings subscription.
struct ConfigSubscription;

/// Long-lived stream that owns the previewer service's D-Bus connection.
fn previewer_stream() -> impl cosmic::iced::futures::Stream<Item = Message> {
    cosmic::iced::stream::channel(16, async move |mut output| {
        use cosmic::iced::futures::SinkExt;

        let Some((service, mut requests)) = previewer::spawn().await else {
            return;
        };

        let service = std::sync::Arc::new(service);
        if output
            .send(Message::PreviewerReady(std::sync::Arc::clone(&service)))
            .await
            .is_err()
        {
            return;
        }

        while let Some(request) = requests.recv().await {
            if output.send(Message::Previewer(request)).await.is_err() {
                break;
            }
        }
    })
}

/// Keyboard handling.
///
/// Lives on the global event listener rather than on a focused widget: the
/// overlay has no text input to focus, and every key here has to work whatever
/// the pointer happens to be over.
fn keys(event: event::Event, _status: event::Status, _id: window::Id) -> Option<Message> {
    match event {
        event::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            match key {
                Key::Named(Named::Escape) => Some(Message::Escape),

                Key::Named(Named::F11) => Some(Message::ToggleFullscreen),

                // Shift turns the horizontal arrows into a scrub, so a video can
                // be seeked without giving up arrow-key navigation between
                // files.
                Key::Named(Named::ArrowLeft) if modifiers.shift() => Some(Message::Seek(-1)),
                Key::Named(Named::ArrowRight) if modifiers.shift() => Some(Message::Seek(1)),

                // The arrows address whatever is on screen. In the index
                // sheet that is the grid's cursor; otherwise it is the
                // selection and the page.
                Key::Named(Named::ArrowLeft) => Some(Message::Navigate(Nav::Left)),
                Key::Named(Named::ArrowRight) => Some(Message::Navigate(Nav::Right)),

                // Vertical arrows turn pages where there are pages to turn, and
                // move between files where there are not — so they do the
                // obvious thing in both cases without a mode.
                Key::Named(Named::ArrowUp | Named::PageUp) => Some(Message::Navigate(Nav::Up)),
                Key::Named(Named::ArrowDown | Named::PageDown) => {
                    Some(Message::Navigate(Nav::Down))
                }

                Key::Named(Named::Enter) => Some(Message::Activate),

                Key::Character(ref c) => match c.as_str() {
                    // Space arrives as a character rather than as a named key.
                    // It closes, the way it does in QuickLook: the key that
                    // opened the preview is the key that takes it away.
                    " " => Some(Message::Escape),
                    "f" => Some(Message::ToggleFullscreen),
                    "c" if modifiers.control() => Some(Message::CopyPath),
                    "+" | "=" => Some(Message::Zoom(1.25)),
                    "-" | "_" => Some(Message::Zoom(0.8)),
                    "0" => Some(Message::ResetZoom),
                    "p" | "k" => Some(Message::TogglePlay),
                    "g" => Some(Message::ToggleGrid),
                    _ => None,
                },

                _ => None,
            }
        }
        event::Event::Window(window::Event::Opened { size, .. }) => Some(Message::Configured(size)),
        event::Event::Window(window::Event::Resized(size)) => Some(Message::Configured(size)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_paths_means_a_plain_activation() {
        use cosmic::app::CosmicFlags;

        let flags = Flags::new(Vec::new());
        assert!(flags.action().is_none(), "an empty invocation is a toggle");
    }

    #[test]
    fn paths_are_forwarded_as_an_action() {
        use cosmic::app::CosmicFlags;

        let flags = Flags::new(vec!["/tmp/a.png".to_owned()]);
        assert_eq!(flags.action().map(String::as_str), Some(SHOW_ACTION));
        assert_eq!(flags.args(), vec!["/tmp/a.png"]);
    }

    #[test]
    fn only_pdfs_have_pages_to_turn() {
        assert_eq!(page_count(&Preview::Pending), 1);
        assert_eq!(page_count(&Preview::Card(Box::default())), 1);
    }
}
