// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! The overlay surface and where the panel sits inside it.
//!
//! Same shape as `light`: one full-screen `wlr-layer-shell` surface on the
//! overlay layer, with the panel drawn *inside* it in surface-local coordinates.
//! A layer surface cannot be transformed by its client and resizing it per frame
//! would round-trip to the compositor for a buffer configure, so anything that
//! animates or resizes has to live inside a surface that does neither.
//!
//! The difference from a launcher is that the panel's size is not a constant.
//! A launcher's panel is a fixed-width list; a previewer's is whatever shape the
//! file is. [`panel_rect`] therefore derives geometry from the preview itself,
//! which is what stops a portrait photograph opening in a landscape frame with
//! empty bars down both sides.

use cosmic::iced::platform_specific::runtime::wayland::layer_surface::{
    IcedMargin, SctkLayerSurfaceSettings,
};
use cosmic::iced::platform_specific::shell::commands::layer_surface::{
    Anchor, KeyboardInteractivity, Layer, destroy_layer_surface, get_layer_surface,
};
use cosmic::iced::{Rectangle, Size, Task, window};
use peek_engine::Preview;

use crate::config::Config;

/// Height of the header strip: file name, type, and the actions.
pub const HEADER_HEIGHT: f32 = 60.0;

/// Height of the transport strip under a media preview.
pub const TRANSPORT_HEIGHT: f32 = 56.0;

/// Corner radius of the panel, used for both the drawn shape and the blur
/// region so the frosted backdrop does not square off the corners.
pub const PANEL_RADIUS: f32 = 18.0;

/// Padding between the panel edge and its content.
pub const PANEL_PADDING: f32 = 16.0;

/// Width of a panel showing a reading column rather than an image.
///
/// Fixed rather than proportional. A line of text 3440 px wide is unreadable
/// however much room the display has, and the point of a text preview is to be
/// read.
pub const READING_WIDTH: f32 = 920.0;

/// Smallest panel the previewer will draw.
///
/// Set by the header, not by the content: the icon, a legible amount of file
/// name, and the four buttons have to fit on one line, and a 16×16 icon
/// previewed at its natural size would otherwise produce a panel narrower than
/// its own title bar.
const MIN_WIDTH: f32 = 520.0;
const MIN_HEIGHT: f32 = 240.0;

/// Fraction of the display height a reading column may occupy.
///
/// Shorter than the width allowance: a full-height column of text against the
/// top and bottom of the screen loses the sense of being an overlay.
const READING_FRACTION: f32 = 0.78;

/// Height of one line in a text preview, in logical pixels.
///
/// Fixed rather than measured, and load-bearing twice over: the renderer
/// windows the document by dividing the scroll position by this, and
/// [`panel_rect`] sizes the panel for a short file by multiplying by it. Both
/// only work because the text widget is given the same value explicitly.
pub const CODE_LINE_HEIGHT: f32 = 18.0;

/// Vertical room reserved for the truncation and lossy-decode notes under a
/// text preview.
const NOTE_HEIGHT: f32 = 28.0;

/// Characters that fit on one wrapped line of prose, approximately.
///
/// Only used to guess how tall a prose document will be, so the panel opens
/// near the right size. A real answer needs the text shaper, which is the
/// frontend's, not this module's.
const PROSE_MEASURE: usize = 90;

/// Create the overlay surface.
///
/// Anchored to all four edges so the compositor stretches it to the full output;
/// `exclusive_zone(-1)` opts out of panel reservation so the surface covers the
/// screen rather than the work area.
pub fn open(id: window::Id) -> Task<()> {
    get_layer_surface(SctkLayerSurfaceSettings {
        id,
        // Overlay rather than Top: a preview is a modal thing and has to sit
        // above the COSMIC panel, the same as the launcher does.
        layer: Layer::Overlay,
        // The previewer owns the keyboard while open — every key it handles
        // (space, escape, the arrows) is a key the file manager underneath also
        // handles, and both acting on one press would be chaos.
        keyboard_interactivity: KeyboardInteractivity::Exclusive,
        anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
        namespace: "peek".into(),
        margin: IcedMargin::default(),
        // Anchored on all sides, so the compositor decides both dimensions.
        size: Some((None, None)),
        // Negative: do not let panels shrink us to the work area.
        exclusive_zone: -1,
        // Accept input across the whole surface, so clicks outside the panel
        // dismiss. `None` means "all input".
        input_zone: None,
        ..Default::default()
    })
}

/// Tear the surface down.
pub fn close(id: window::Id) -> Task<()> {
    destroy_layer_surface(id)
}

/// How the overlay is currently being shown.
///
/// The three pieces of view state that change geometry, passed together so a
/// function that needs one of them does not grow a positional `bool` per
/// question — and so adding a fourth mode later touches one struct rather
/// than six signatures.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub zoom: f32,
    /// The content has the whole output: no header, no panel, no margin.
    pub fullscreen: bool,
    /// The settings list is showing instead of the file.
    pub settings: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            fullscreen: false,
            settings: false,
        }
    }
}

/// The size a visual preview naturally occupies, before zoom.
///
/// `None` for anything that is not visual — a reading column's width is a
/// legibility decision, not a scaling one, so none of this applies to it.
///
/// Fullscreen is not a different layout, only a different allowance: the
/// content is fitted the same way, against all of the display rather than the
/// configured fraction of it.
#[must_use]
pub fn fit_size(screen: Size, preview: &Preview, config: &Config, layout: Layout) -> Option<Size> {
    let fullscreen = layout.fullscreen;
    let aspect = preview.aspect()?;
    if !aspect.is_finite() || aspect <= 0.0 {
        return None;
    }

    let fraction = if fullscreen { 1.0 } else { config.fraction() };
    let available_width = (screen.width * fraction).max(MIN_WIDTH);
    let available_height = (screen.height * fraction).max(MIN_HEIGHT);

    let chrome = if fullscreen {
        // The header goes; the transport stays, because a video with no way
        // to pause it is not a preview, it is a screensaver.
        transport_height(preview)
    } else {
        HEADER_HEIGHT + transport_height(preview) + PANEL_PADDING * 2.0
    };
    let for_content = (available_height - chrome).max(MIN_HEIGHT);

    // Both dimensions are constrained, so a panorama is limited by width and a
    // portrait by height without either being special-cased.
    let by_width = (available_width, available_width / aspect);
    let by_height = (for_content * aspect, for_content);

    let (mut width, mut height) = if by_width.1 <= for_content {
        by_width
    } else {
        by_height
    };

    // A raster has a finite resolution, and stretching it past that shows the
    // scaler rather than the file — a 120×120 icon filling a 1400 px panel is a
    // blurred square, not a preview. Fitting is therefore capped at the
    // content's own size. Zoom is applied afterwards, so magnifying past it
    // stays available as something the user asked for rather than something
    // that happens by default.
    // Fullscreen is an explicit request for the largest view, so the
    // natural-size cap — which exists to stop *accidental* upscaling — does
    // not apply to it.
    if let Some((natural_width, natural_height)) = preview.natural_size()
        && !fullscreen
        && (natural_width as f32) < width
    {
        width = natural_width as f32;
        height = natural_height as f32;
    }

    Some(Size::new(width, height))
}

/// The size a visual preview is *drawn* at: the fit, magnified.
///
/// Unclamped on purpose. Past the display size the image no longer fits its
/// viewport, and the difference between this and [`content_size`] is what the
/// view crops and the user pans through.
#[must_use]
pub fn image_size(
    screen: Size,
    preview: &Preview,
    config: &Config,
    layout: Layout,
) -> Option<Size> {
    let fit = fit_size(screen, preview, config, layout)?;
    let zoom = layout.zoom.clamp(MIN_ZOOM, MAX_ZOOM);
    Some(Size::new(fit.width * zoom, fit.height * zoom))
}

/// The viewport a visual preview is shown through.
///
/// Both the panel geometry and the image widget derive from this. They have to:
/// the panel is sized around the content, and the widget is drawn at the size
/// the panel was built for. Computing it twice is how they drift apart.
#[must_use]
pub fn content_size(
    screen: Size,
    preview: &Preview,
    config: &Config,
    layout: Layout,
) -> Option<Size> {
    let image = image_size(screen, preview, config, layout)?;
    if layout.fullscreen {
        // The viewport is the output. No margin: the margin is the panel's,
        // and in this mode there is no panel.
        return Some(Size::new(
            image.width.min(screen.width),
            image.height.min(screen.height - transport_height(preview)),
        ));
    }
    let chrome = HEADER_HEIGHT + transport_height(preview) + PANEL_PADDING * 2.0;
    Some(Size::new(
        image.width.min(screen.width - 24.0),
        image.height.min(screen.height - chrome - 24.0),
    ))
}

/// How far the content can be panned from centre, per axis.
///
/// Zero while the image fits its viewport, which is what makes panning inert
/// until zoom actually crops something.
#[must_use]
pub fn max_pan(screen: Size, preview: &Preview, config: &Config, layout: Layout) -> (f32, f32) {
    match (
        image_size(screen, preview, config, layout),
        content_size(screen, preview, config, layout),
    ) {
        (Some(image), Some(viewport)) => (
            ((image.width - viewport.width) / 2.0).max(0.0),
            ((image.height - viewport.height) / 2.0).max(0.0),
        ),
        _ => (0.0, 0.0),
    }
}

/// Where the panel sits inside the full-screen surface.
///
/// The settings list is a reading column whatever the file behind it is:
/// sizing the panel to a photograph's aspect ratio and then filling it with
/// toggles is how a settings list ends up in a letterbox.
#[must_use]
pub fn panel_rect(screen: Size, preview: &Preview, config: &Config, layout: Layout) -> Rectangle {
    let fullscreen = layout.fullscreen;
    if layout.settings {
        let width = READING_WIDTH.min(screen.width - 48.0).max(MIN_WIDTH);
        let height = (screen.height * READING_FRACTION).max(MIN_HEIGHT);
        return centred(screen, width, height);
    }

    let (width, height) = match content_size(screen, preview, config, layout) {
        Some(content) if fullscreen => (content.width, content.height + transport_height(preview)),
        Some(content) => (
            content.width,
            content.height + HEADER_HEIGHT + transport_height(preview) + PANEL_PADDING * 2.0,
        ),
        // A preview with no aspect ratio — text, a card — fills the output in
        // fullscreen rather than keeping its reading column, because the mode
        // was asked for explicitly.
        None if fullscreen => (screen.width, screen.height),
        // Everything else is a reading column.
        None => {
            let tallest = (screen.height * READING_FRACTION).max(MIN_HEIGHT);
            let height = match preview {
                // Sized to the file, within limits. Most source files exceed
                // the ceiling, so arrowing through a project keeps a steady
                // panel — but a three-line note no longer floats in a column of
                // empty space.
                Preview::Text(document) => {
                    let notes = if document.truncated || document.lossy {
                        NOTE_HEIGHT
                    } else {
                        0.0
                    };

                    let rows: f32 = if document.prose {
                        // Prose wraps, so a paragraph is taller than one row.
                        // Estimated rather than measured: the real height needs
                        // the shaper, and this only decides the panel's size —
                        // the content scrolls either way, so being a row out
                        // costs nothing.
                        document
                            .lines
                            .iter()
                            .map(|line| {
                                let characters = line.plain().chars().count();
                                (characters.div_ceil(PROSE_MEASURE)).max(1) as f32
                            })
                            .sum()
                    } else {
                        document.lines.len() as f32
                    };

                    let content = rows * CODE_LINE_HEIGHT + notes;
                    (content + HEADER_HEIGHT + PANEL_PADDING * 2.0).clamp(MIN_HEIGHT, tallest)
                }
                _ => tallest,
            };
            (READING_WIDTH.min(screen.width - 48.0), height)
        }
    };

    let width = width
        .max(if fullscreen { 0.0 } else { MIN_WIDTH })
        .min(screen.width);
    let height = height
        .max(if fullscreen { 0.0 } else { MIN_HEIGHT })
        .min(screen.height);

    centred(screen, width, height)
}

/// Place a panel of this size in the middle of the output.
///
/// Geometrically centred, unlike the launcher's optical bias. A launcher is
/// read top-down from a fixed input field; a preview is looked *at*, and an
/// image sitting above centre reads as misplaced rather than as poised.
fn centred(screen: Size, width: f32, height: f32) -> Rectangle {
    Rectangle {
        x: ((screen.width - width) / 2.0).max(0.0),
        y: ((screen.height - height) / 2.0).max(0.0),
        width,
        height,
    }
}

/// Smallest and largest zoom the panel will take.
///
/// Bounded on both sides: below the minimum the panel hits [`MIN_WIDTH`] and
/// stops responding, which reads as the control being broken, and above the
/// maximum the panel is larger than the display and the content is cropped
/// rather than magnified.
pub const MIN_ZOOM: f32 = 0.4;
pub const MAX_ZOOM: f32 = 3.0;

/// How much vertical room a preview's transport controls need.
///
/// Zero for everything that is not playable, which is why this is a function
/// rather than a constant added everywhere.
#[must_use]
pub fn transport_height(preview: &Preview) -> f32 {
    match preview {
        Preview::Media(media) if media.decodable => TRANSPORT_HEIGHT,
        _ => 0.0,
    }
}

/// Edge of one index-sheet cell, in logical pixels.
pub const CELL_EDGE: f32 = 132.0;

/// Gap between index-sheet cells.
pub const CELL_SPACING: f32 = 10.0;

/// How many cells the index sheet fits across a panel of this width.
///
/// One function rather than two derivations: the view lays the sheet out with
/// it and the up arrow moves by it, and those must agree or vertical movement
/// jumps by the wrong amount.
#[must_use]
pub fn grid_columns(panel_width: f32) -> usize {
    let content = (panel_width - PANEL_PADDING * 2.0).max(CELL_EDGE);
    ((content / (CELL_EDGE + CELL_SPACING)).floor() as usize).max(1)
}

/// Ask the compositor to blur the backdrop behind the panel.
///
/// Uses `ext-background-effect-v1`. libcosmic no-ops when the compositor does
/// not advertise the protocol, so this needs no fallback path — the panel simply
/// renders as a plain translucent surface.
///
/// The region is surface-local double-buffered state, so it has to be resent
/// whenever the panel geometry changes, which for a previewer is on every file.
pub fn set_blur(id: window::Id, panel: Rectangle, enabled: bool) -> Task<()> {
    let regions = if enabled { Some(vec![panel]) } else { None };
    cosmic::iced::platform_specific::shell::commands::blur::blur(id, regions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Size {
        Size::new(1920.0, 1080.0)
    }

    fn picture(aspect: f32) -> Preview {
        // A 1×N raster is the cheapest way to express an aspect ratio without
        // allocating a real image.
        let height = 100;
        let width = (100.0 * aspect).round() as u32;
        Preview::Picture(Box::new(peek_engine::Picture {
            raster: peek_engine::Raster::new(width, height, vec![0; (width * height * 4) as usize])
                .expect("raster"),
            source_width: width,
            source_height: height,
            exif: Vec::new(),
            // Scalable, so these tests exercise the fit-to-panel path rather
            // than the natural-size cap; that cap has tests of its own below.
            scalable: true,
            animation: None,
        }))
    }

    #[test]
    fn a_portrait_image_gets_a_portrait_panel() {
        let rect = panel_rect(
            screen(),
            &picture(0.5),
            &Config::default(),
            Layout::default(),
        );
        assert!(
            rect.height > rect.width,
            "expected a taller-than-wide panel, got {rect:?}"
        );
    }

    #[test]
    fn a_wide_image_is_limited_by_the_display_width() {
        let rect = panel_rect(
            screen(),
            &picture(4.0),
            &Config::default(),
            Layout::default(),
        );
        assert!(rect.width <= screen().width);
        assert!(rect.height <= screen().height);
    }

    #[test]
    fn a_reading_column_ignores_the_content_and_uses_a_fixed_width() {
        let rect = panel_rect(
            screen(),
            &Preview::Card(Box::default()),
            &Config::default(),
            Layout::default(),
        );
        assert_eq!(rect.width, READING_WIDTH);
    }

    /// A raster, which carries a real resolution and must not be enlarged past it.
    fn photo(width: u32, height: u32) -> Preview {
        Preview::Picture(Box::new(peek_engine::Picture {
            raster: peek_engine::Raster::new(width, height, vec![0; (width * height * 4) as usize])
                .expect("raster"),
            source_width: width,
            source_height: height,
            exif: Vec::new(),
            scalable: false,
            animation: None,
        }))
    }

    #[test]
    fn a_small_image_is_drawn_at_its_own_size() {
        let content = content_size(
            screen(),
            &photo(120, 120),
            &Config::default(),
            Layout::default(),
        )
        .expect("a visual preview has a content size");

        assert_eq!(content.width, 120.0);
        assert_eq!(content.height, 120.0);
    }

    #[test]
    fn a_small_image_still_gets_a_usable_panel_around_it() {
        // The content shrinks to 120px but the panel keeps its minimum, so the
        // header and its buttons still have room.
        let rect = panel_rect(
            screen(),
            &photo(120, 120),
            &Config::default(),
            Layout::default(),
        );
        assert!(rect.width >= MIN_WIDTH);
        assert!(rect.height >= MIN_HEIGHT);
    }

    #[test]
    fn zoom_still_magnifies_a_small_image() {
        let base = content_size(
            screen(),
            &photo(120, 120),
            &Config::default(),
            Layout::default(),
        )
        .expect("content size");
        let zoomed = content_size(
            screen(),
            &photo(120, 120),
            &Config::default(),
            Layout {
                zoom: 2.0,
                ..Layout::default()
            },
        )
        .expect("content size");

        assert!(
            zoomed.width > base.width,
            "the natural-size cap must not disable zoom"
        );
    }

    #[test]
    fn a_large_image_is_still_fitted_down_to_the_display() {
        let content = content_size(
            screen(),
            &photo(8000, 4000),
            &Config::default(),
            Layout::default(),
        )
        .expect("content size");

        assert!(content.width < 8000.0, "an oversized photo must be fitted");
        assert!((content.width / content.height - 2.0).abs() < 0.05);
    }

    #[test]
    fn a_vector_is_allowed_to_fill_the_panel() {
        // `scalable`, so the cap does not apply: rendering an SVG at its
        // document size and leaving it there is the failure this avoids.
        let content = content_size(
            screen(),
            &picture(1.0),
            &Config::default(),
            Layout::default(),
        )
        .expect("content size");
        assert!(
            content.width > 200.0,
            "a vector should fill, got {content:?}"
        );
    }

    #[test]
    fn the_panel_is_centred() {
        let rect = panel_rect(
            screen(),
            &picture(1.0),
            &Config::default(),
            Layout::default(),
        );
        let left = rect.x;
        let right = screen().width - (rect.x + rect.width);
        assert!((left - right).abs() < 1.0, "expected a centred panel");
    }

    #[test]
    fn zoom_never_pushes_the_panel_off_the_display() {
        let rect = panel_rect(
            screen(),
            &picture(1.0),
            &Config::default(),
            Layout {
                zoom: MAX_ZOOM,
                ..Layout::default()
            },
        );
        assert!(rect.x >= 0.0 && rect.y >= 0.0);
        assert!(rect.x + rect.width <= screen().width + 1.0);
        assert!(rect.y + rect.height <= screen().height + 1.0);
    }

    /// A source preview with the given number of one-span lines.
    fn text(lines: usize) -> Preview {
        Preview::Text(Box::new(peek_engine::Document {
            lines: vec![peek_engine::text::Line::default(); lines],
            ..peek_engine::Document::default()
        }))
    }

    /// A prose preview whose lines are long enough to wrap.
    fn prose(lines: usize, characters: usize) -> Preview {
        let line = peek_engine::text::Line {
            spans: vec![peek_engine::text::Span {
                text: "x".repeat(characters),
                color: [0, 0, 0],
                bold: false,
                italic: false,
            }],
        };
        Preview::Text(Box::new(peek_engine::Document {
            lines: vec![line; lines],
            prose: true,
            ..peek_engine::Document::default()
        }))
    }

    #[test]
    fn a_short_text_file_gets_a_short_panel() {
        let rect = panel_rect(screen(), &text(3), &Config::default(), Layout::default());
        assert_eq!(
            rect.height, MIN_HEIGHT,
            "three lines must not float in a full reading column"
        );
    }

    #[test]
    fn a_long_text_file_still_hits_the_reading_ceiling() {
        let rect = panel_rect(
            screen(),
            &text(4_000),
            &Config::default(),
            Layout::default(),
        );
        let ceiling = screen().height * READING_FRACTION;
        assert!((rect.height - ceiling).abs() < 1.0, "got {}", rect.height);
    }

    #[test]
    fn wrapped_prose_is_given_room_for_its_wrapping() {
        // Ten paragraphs that each wrap to about four rows must not be sized
        // as ten rows, or the panel opens a quarter of the height it needs.
        let wrapped = panel_rect(
            screen(),
            &prose(10, 360),
            &Config::default(),
            Layout::default(),
        );
        let unwrapped = panel_rect(screen(), &text(10), &Config::default(), Layout::default());
        assert!(
            wrapped.height > unwrapped.height,
            "wrapping must add height: {} vs {}",
            wrapped.height,
            unwrapped.height
        );
    }

    #[test]
    fn panning_is_inert_until_the_image_is_cropped() {
        let fitted = max_pan(
            screen(),
            &photo(800, 600),
            &Config::default(),
            Layout::default(),
        );
        assert_eq!(fitted, (0.0, 0.0), "an image that fits has nowhere to pan");

        let (x, y) = max_pan(
            screen(),
            &photo(8000, 4000),
            &Config::default(),
            Layout {
                zoom: MAX_ZOOM,
                ..Layout::default()
            },
        );
        assert!(x > 0.0 && y > 0.0, "a cropped image pans on both axes");
    }

    #[test]
    fn the_index_sheet_fits_whole_cells_and_never_fewer_than_one() {
        assert!(
            grid_columns(READING_WIDTH) >= 4,
            "a reading column fits several"
        );
        // Narrower than one cell still lays out, rather than dividing by zero
        // or producing an empty row.
        assert_eq!(grid_columns(10.0), 1);
    }

    #[test]
    fn the_settings_list_gets_a_reading_column_whatever_is_behind_it() {
        // A wide photograph behind the settings must not letterbox them.
        let rect = panel_rect(
            screen(),
            &picture(4.0),
            &Config::default(),
            Layout {
                settings: true,
                ..Layout::default()
            },
        );
        assert_eq!(rect.width, READING_WIDTH);
        assert!(rect.height > MIN_HEIGHT);
    }

    #[test]
    fn fullscreen_gives_the_content_the_whole_output() {
        let windowed = panel_rect(
            screen(),
            &picture(1.5),
            &Config::default(),
            Layout::default(),
        );
        let full = panel_rect(
            screen(),
            &picture(1.5),
            &Config::default(),
            Layout {
                fullscreen: true,
                ..Layout::default()
            },
        );

        assert!(full.width > windowed.width, "fullscreen is larger");
        assert!(full.height > windowed.height);
        assert!(full.width <= screen().width && full.height <= screen().height);
    }

    #[test]
    fn fullscreen_shows_a_small_image_larger_than_its_own_pixels() {
        // The natural-size cap stops *accidental* upscaling; asking for
        // fullscreen is not accidental.
        let windowed = content_size(
            screen(),
            &photo(120, 120),
            &Config::default(),
            Layout::default(),
        )
        .expect("content size");
        let full = content_size(
            screen(),
            &photo(120, 120),
            &Config::default(),
            Layout {
                fullscreen: true,
                ..Layout::default()
            },
        )
        .expect("content size");
        assert_eq!(windowed.width, 120.0);
        assert!(full.width > 120.0, "got {full:?}");
    }

    #[test]
    fn text_fills_the_output_in_fullscreen_rather_than_keeping_its_column() {
        let full = panel_rect(
            screen(),
            &text(10),
            &Config::default(),
            Layout {
                fullscreen: true,
                ..Layout::default()
            },
        );
        assert_eq!(full.width, screen().width);
        assert_eq!(full.height, screen().height);
    }

    #[test]
    fn the_drawn_image_and_the_viewport_agree_until_zoom_crops() {
        let image = image_size(
            screen(),
            &photo(800, 600),
            &Config::default(),
            Layout::default(),
        )
        .expect("size");
        let viewport = content_size(
            screen(),
            &photo(800, 600),
            &Config::default(),
            Layout::default(),
        )
        .expect("size");
        assert_eq!(image, viewport);
    }
}
