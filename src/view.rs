// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Rendering.
//!
//! ## How fading works
//!
//! iced has no generic opacity wrapper — only `image` and `svg` expose one — so
//! the panel cannot be faded as a unit. Every colour this module produces goes
//! through [`fade`], which scales its alpha by the current transition progress.
//! More bookkeeping than a compositor opacity would be, and the only approach
//! that fades text, borders, and backgrounds together.
//!
//! ## Why a text preview is one widget
//!
//! A syntax-highlighted file is thousands of lines of several coloured runs
//! each. Building a widget per run means tens of thousands of widgets in a
//! scrollable that lays out all of its children, which is slow enough to be felt
//! on every keystroke. Instead the whole document becomes a single `rich_text`
//! whose spans carry the newlines, so the text shaper sees one paragraph and the
//! widget tree sees one node.

use std::time::Instant;

use cosmic::iced::mouse::ScrollDelta;
use cosmic::iced::widget::text::{LineHeight, Wrapping};
use cosmic::iced::widget::{pin, rich_text, space, span};
use cosmic::iced::{Alignment, Background, Color, ContentFit, Length, Size, Vector};
use cosmic::widget::{
    button, column, container, icon, image, mouse_area, row, scrollable, slider, text,
};
use cosmic::{Apply, Element};
use peek_engine::preview::Reason;
use peek_engine::{Entry, Neighbourhood, Player, Preview, Raster, meta};

use crate::anim::Panel;
use crate::app::{Message, Setting};
use crate::config::Config;
use crate::fl;
use crate::surface::{self, HEADER_HEIGHT, PANEL_PADDING, PANEL_RADIUS, TRANSPORT_HEIGHT};

/// Edge length of the file-type icon in the header.
const HEADER_ICON: u16 = 30;

/// Point size of a source preview.
const CODE_SIZE: u16 = 13;

/// Point size of a prose preview.
///
/// Larger than source: prose is read continuously and source is scanned, and
/// the reading column's measure was chosen for a size like this.
const PROSE_SIZE: u16 = 15;

/// Opacity of the panel's secondary text — subtitles, line numbers, hints.
const MUTED: f32 = 0.62;

/// Height an ebook's cover is drawn at, above its opening text.
const COVER_HEIGHT: f32 = 260.0;

use crate::surface::{CELL_EDGE, CELL_SPACING};

/// Height a cell gives its thumbnail, above the file name.
const THUMBNAIL_EDGE: f32 = 108.0;

/// Width of a slider in the settings panel.
const SLIDER_WIDTH: f32 = 220.0;

/// Sizes the specimen pangram is set at, largest first.
///
/// A ladder rather than one size: what a face does at 32 pt and what it does
/// at 11 pt are different questions, and someone opening a font is asking
/// both.
const SPECIMEN_SIZES: &[u16] = &[32, 24, 18, 14, 11];

/// Everything the overlay needs to draw one frame.
///
/// Passed as a struct rather than as fifteen positional arguments, which is
/// where `light`'s equivalent ended up and is the one thing worth doing
/// differently.
pub struct Frame<'a> {
    pub panel: &'a Panel,
    pub now: Instant,
    pub screen: Size,
    pub preview: &'a Preview,
    pub entry: Option<&'a Entry>,
    /// GPU handle for the current image, built when the preview loaded.
    pub image: Option<&'a image::Handle>,
    pub around: &'a Neighbourhood,
    pub player: Option<&'a Player>,
    pub zoom: f32,
    /// How far a cropped visual preview is panned from centre.
    pub pan: Vector,
    /// Absolute scroll offset of a text preview, for windowing the document.
    pub text_scroll: f32,
    /// Whether the current image is playing as an animation. Carried here
    /// because the frames themselves live with the application as GPU handles,
    /// not in the preview.
    pub animated: bool,
    /// Whether the content has been given the whole output.
    pub fullscreen: bool,
    /// Whether the settings panel is showing instead of the preview.
    pub settings: bool,
    /// Whether the index sheet is showing instead of one file.
    pub grid: bool,
    /// Which cell the index sheet's cursor is on.
    pub grid_index: usize,
    /// Rendered cells, keyed by path. A present `None` is a file that has no
    /// thumbnail; an absent key is one still rendering.
    pub thumbnails: &'a std::collections::HashMap<std::path::PathBuf, Option<image::Handle>>,
    pub config: &'a Config,
}

impl Frame<'_> {
    /// How the overlay is being shown, for the geometry functions.
    fn layout(&self) -> surface::Layout {
        surface::Layout {
            zoom: self.zoom,
            fullscreen: self.fullscreen,
            settings: self.settings,
        }
    }
}

/// Build the image handle a preview will draw, if it draws one.
///
/// Called once when a preview lands rather than on every frame: iced derives a
/// handle's identity by hashing its bytes, and a 4K frame is 33 MB to hash.
#[must_use]
pub fn handle_for(preview: &Preview) -> Option<image::Handle> {
    match preview {
        Preview::Picture(picture) => Some(handle_for_raster(&picture.raster)),
        Preview::Pdf(pdf) => Some(handle_for_raster(&pdf.raster)),
        Preview::Comic(comic) => Some(handle_for_raster(&comic.raster)),
        Preview::Ebook(book) => book.cover.as_ref().map(handle_for_raster),
        Preview::Media(media) => media.poster.as_ref().map(handle_for_raster),
        _ => None,
    }
}

/// Wrap a raster as an iced image handle, copying its pixels.
///
/// For previews that stay in application state and are drawn many times.
#[must_use]
pub fn handle_for_raster(raster: &Raster) -> image::Handle {
    image::Handle::from_rgba(raster.width, raster.height, raster.pixels.clone())
}

/// Wrap a raster as an iced image handle, taking its buffer.
///
/// The video path decodes a fresh frame for every handle it builds and keeps no
/// other reference to it. `Bytes::from(Vec<u8>)` takes ownership without
/// copying, so handing the buffer over rather than cloning it removes a
/// full-frame memcpy from every drawn frame — eight megabytes at 1080p, at
/// display rate, on the thread that also has to answer the arrow keys.
#[must_use]
pub fn handle_from_raster(raster: Raster) -> image::Handle {
    image::Handle::from_rgba(raster.width, raster.height, raster.pixels)
}

/// A pixel or point size, as the header and the media card both show it.
fn dimensions(width: u32, height: u32) -> String {
    fl!("dimensions", width = width, height = height)
}

/// How many files and folders a directory holds.
///
/// One function for the two places that say it, so the header summary and the
/// directory card cannot drift into two different phrasings of the same fact.
fn folder_summary(directory: &peek_engine::Directory) -> String {
    fl!(
        "folder-summary",
        files = directory.files,
        folders = directory.directories
    )
}

/// Present a count to Fluent in a form its plural rules can select on.
///
/// Fluent picks the `[one]` / `[other]` variant from the *number*, so the count
/// has to arrive as one rather than as text — "1 minutes ago" is what happens
/// otherwise. `f64` because that is what `FluentNumber` stores internally, and
/// converting once here is clearer than letting each call site guess.
fn plural_count(count: u64) -> f64 {
    count as f64
}

/// A readable name for a MIME type, in the user's language.
///
/// The engine hands over the type itself and the displayable core of its
/// subtype; the category words — "image", "archive", "document" — are chosen
/// here, because they are prose. The fallback keeps the subtype rather than
/// saying "Unknown", which discards information the user can act on.
fn mime_name(mime: &str) -> String {
    match mime {
        "inode/directory" => return fl!("mime-folder"),
        "application/pdf" | "application/x-pdf" => return fl!("mime-pdf"),
        "application/zip" => return fl!("mime-zip"),
        "text/plain" => return fl!("mime-plain-text"),
        "application/octet-stream" => return fl!("mime-binary"),
        _ => {}
    }

    let subtype = meta::mime_subtype(mime);
    match mime.split('/').next().unwrap_or_default() {
        "image" => fl!("mime-image", subtype = subtype),
        "video" => fl!("mime-video", subtype = subtype),
        "audio" => fl!("mime-audio", subtype = subtype),
        "text" => fl!("mime-text", subtype = subtype),
        "font" => fl!("mime-font", subtype = subtype),
        _ => fl!("mime-file", subtype = subtype),
    }
}

/// Why a previewer declined, worded.
fn reason_text(reason: Reason) -> String {
    match reason {
        Reason::Unreadable => fl!("reason-unreadable"),
        Reason::Undecodable => fl!("reason-undecodable"),
        Reason::Encrypted => fl!("reason-encrypted"),
        Reason::TooLarge => fl!("reason-too-large"),
        Reason::Empty => fl!("reason-empty"),
        // The same fact the media panel states for a probed file with no
        // usable streams, so it uses the same words.
        Reason::NoCodec => fl!("no-codec"),
        Reason::NoPreview => fl!("reason-no-preview"),
    }
}

/// The label for a metadata row.
///
/// The engine reports *which* fact a row states; the word for it is chosen
/// here, because it is the only layer that knows what language the user reads.
fn field_label(field: meta::Field) -> String {
    match field {
        meta::Field::Kind => fl!("field-kind"),
        meta::Field::Type => fl!("field-type"),
        meta::Field::Size => fl!("field-size"),
        meta::Field::Modified => fl!("field-modified"),
        meta::Field::Permissions => fl!("field-permissions"),
        meta::Field::SymlinkTo => fl!("field-symlink-to"),
        meta::Field::Where => fl!("field-where"),
    }
}

/// The value of a metadata row, worded.
fn field_value(value: &meta::Value) -> String {
    match value {
        meta::Value::Text(text) => text.clone(),
        meta::Value::Mime(mime) => mime_name(mime),
        // Grouped digits: an exact byte count is the reason to show this row at
        // all, and 1073741824 is unreadable without them.
        meta::Value::Bytes(bytes) => fl!(
            "size-with-bytes",
            size = meta::size(*bytes),
            bytes = meta::group_digits(*bytes)
        ),
        meta::Value::Elapsed(elapsed) => match elapsed {
            meta::Elapsed::JustNow => fl!("just-now"),
            meta::Elapsed::Minutes(count) => fl!("minutes-ago", count = plural_count(*count)),
            meta::Elapsed::Hours(count) => fl!("hours-ago", count = plural_count(*count)),
            meta::Elapsed::Days(count) => fl!("days-ago", count = plural_count(*count)),
            meta::Elapsed::On(date) => fl!("on-date", date = date.clone()),
            meta::Elapsed::BeforeEpoch => fl!("before-epoch"),
        },
    }
}

/// Scale a colour's alpha, for transition fades.
fn fade(color: Color, alpha: f32) -> Color {
    Color {
        a: color.a * alpha.clamp(0.0, 1.0),
        ..color
    }
}

/// The whole overlay: a transparent full-screen layer with the panel pinned
/// inside it.
pub fn overlay(frame: Frame<'_>) -> Element<'_, Message> {
    let alpha = frame.panel.opacity(frame.now);
    let rect = surface::panel_rect(frame.screen, frame.preview, frame.config, frame.layout());

    let pinned = pin(panel(&frame, alpha, rect))
        .x(rect.x)
        // The rise is applied to the pin origin rather than as padding: moving
        // an origin costs nothing, whereas animating padding relayouts the
        // whole panel on every frame.
        .y(rect.y + frame.panel.offset_y(frame.now));

    let backdrop = container(pinned)
        .width(Length::Fill)
        .height(Length::Fill)
        .class(cosmic::theme::Container::Transparent);

    if frame.config.click_away {
        // The backdrop covers the output and swallows clicks, giving click-away
        // dismissal without a separate input region.
        mouse_area(backdrop).on_press(Message::Dismiss).into()
    } else {
        backdrop.into()
    }
}

/// The panel: header, content, and — for media — a transport strip.
fn panel<'a>(frame: &Frame<'a>, alpha: f32, rect: cosmic::iced::Rectangle) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();

    let transport_height = surface::transport_height(frame.preview);
    let chrome = if frame.fullscreen {
        transport_height
    } else {
        HEADER_HEIGHT + transport_height + PANEL_PADDING
    };
    let content_height = (rect.height - chrome).max(1.0);

    // Fullscreen drops the header and the panel's own surface: the point of
    // the mode is that nothing but the file is on screen. The transport stays
    // where there is one — a video with no way to pause it is not a preview.
    let mut children: Vec<Element<'a, Message>> = if frame.fullscreen {
        vec![content(frame, alpha, rect.width, content_height)]
    } else {
        vec![
            header(frame, alpha),
            content(frame, alpha, rect.width, content_height),
        ]
    };

    if transport_height > 0.0 {
        children.push(transport(frame, alpha));
    }

    let (fill, border) = if frame.fullscreen {
        // Transparent: the compositor's own backdrop is a better ground for a
        // photograph than a tinted panel is, and there is no panel edge left
        // to draw.
        (Color::TRANSPARENT, Color::TRANSPARENT)
    } else {
        (
            fade(
                Color::from(cosmic.bg_color()),
                alpha * frame.config.panel_opacity(),
            ),
            fade(Color::from(cosmic.bg_divider()), alpha),
        )
    };
    let radius = if frame.fullscreen { 0.0 } else { PANEL_RADIUS };

    column::with_children(children)
        .width(Length::Fixed(rect.width))
        .height(Length::Fixed(rect.height))
        .apply(container)
        .class(cosmic::theme::Container::custom(move |_| {
            cosmic::iced::widget::container::Style {
                background: Some(Background::Color(fill)),
                border: cosmic::iced::Border {
                    radius: radius.into(),
                    width: if radius > 0.0 { 1.0 } else { 0.0 },
                    color: border,
                },
                ..Default::default()
            }
        }))
        .into()
}

/// The header strip: icon, name, a one-line summary, and the actions.
fn header<'a>(frame: &Frame<'a>, alpha: f32) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();
    let body = fade(Color::from(cosmic.on_bg_color()), alpha);
    let muted = fade(Color::from(cosmic.on_bg_color()), alpha * MUTED);

    let (name, icon_name) = if frame.grid {
        (fl!("index-sheet"), "view-grid-symbolic".to_owned())
    } else if frame.settings {
        (
            fl!("settings-appearance"),
            "emblem-system-symbolic".to_owned(),
        )
    } else {
        match frame.entry {
            Some(entry) => (entry.name.clone(), meta::icon_name(&entry.mime)),
            None => (fl!("no-file"), "text-x-generic".to_owned()),
        }
    };

    // Both names are offered to the icon theme: the specific one first, then the
    // category's generic. Icon themes are patchy about which MIME types they
    // cover, and a missing specific icon should fall back rather than render as
    // a blank.
    let fallback = frame.entry.map_or("text-x-generic", |entry| {
        meta::generic_icon_name(&entry.mime)
    });

    let type_icon = icon::from_name(icon_name)
        .fallback(Some(icon::IconFallback::Names(vec![fallback.into()])))
        .size(HEADER_ICON)
        .icon();

    // Neither line wraps. The summary is long — type, size, dimensions, and
    // position in the directory — and a wrapping header would either grow past
    // `HEADER_HEIGHT` or push the buttons off the panel, which is what happens
    // at the minimum width for a small image.
    let mut titles = column::with_capacity(2)
        .push(
            text::body(name)
                .size(15)
                .wrapping(Wrapping::None)
                .class(colour(body)),
        )
        .spacing(1);

    if let Some(summary) = summary(frame) {
        titles = titles.push(
            text::caption(summary)
                .wrapping(Wrapping::None)
                .class(colour(muted)),
        );
    }

    row::with_capacity(3)
        .push(type_icon)
        // `Fill` rather than a spacer beside it: the titles take whatever is
        // left after the icon and the buttons, so the buttons keep their room
        // however long the file name is.
        .push(container(titles).width(Length::Fill).clip(true))
        .push(actions(frame, alpha))
        .spacing(12)
        .align_y(Alignment::Center)
        .apply(container)
        .padding([0, PANEL_PADDING as u16])
        .height(Length::Fixed(HEADER_HEIGHT))
        .width(Length::Fill)
        .into()
}

/// The one-line description under the file name.
///
/// Composed from whatever the preview actually knows, so an image reports its
/// pixel dimensions and a PDF reports its page count without either needing a
/// dedicated header layout.
fn summary(frame: &Frame<'_>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    if frame.grid {
        // The sheet describes the selection, not the file behind it.
        let (_, total) = frame.around.position();
        return Some(fl!("item-count", count = plural_count(total as u64)));
    }

    if let Some(entry) = frame.entry {
        parts.push(mime_name(&entry.mime));
        if entry.size > 0 {
            parts.push(meta::size(entry.size));
        }
    }

    match frame.preview {
        Preview::Picture(picture) => {
            parts.push(dimensions(picture.source_width, picture.source_height));
            if frame.animated || picture.animation.is_some() {
                parts.push(fl!("animated"));
            }
        }
        Preview::Pdf(pdf) => parts.push(fl!(
            "page-of",
            page = pdf.page.saturating_add(1),
            pages = pdf.pages
        )),
        Preview::Comic(comic) => parts.push(fl!(
            "page-of",
            page = comic.page.saturating_add(1),
            pages = comic.pages
        )),
        Preview::Ebook(book) => {
            if !book.authors.is_empty() {
                parts.push(book.authors.join(", "));
            }
            parts.push(fl!("line-count", count = book.lines.len()));
        }
        Preview::Font(font) => {
            if let Some(style) = &font.style {
                parts.push(style.clone());
            }
            parts.push(fl!(
                "glyph-count",
                count = plural_count(u64::from(font.glyphs))
            ));
        }
        Preview::Text(document) => {
            if let Some(language) = &document.language {
                parts.push(language.clone());
            }
            parts.push(fl!("line-count", count = document.lines.len()));
        }
        Preview::Archive(archive) => parts.push(fl!(
            "archive-summary",
            size = archive.format.clone(),
            count = archive.members.len()
        )),
        Preview::Media(media) => {
            if let Some(duration) = media.duration {
                parts.push(meta::duration(duration));
            }
            if media.has_video && media.height > 0 {
                parts.push(dimensions(media.width, media.height));
            }
        }
        Preview::Directory(directory) => parts.push(folder_summary(directory)),
        _ => {}
    }

    // Position within the neighbourhood, so holding an arrow key shows progress
    // through the directory rather than an unchanging header.
    let (current, total) = frame.around.position();
    if total > 1 {
        parts.push(fl!("position-of", current = current, total = total));
    }

    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Header buttons.
fn actions<'a>(frame: &Frame<'a>, alpha: f32) -> Element<'a, Message> {
    let mut controls = row::with_capacity(3).spacing(4).align_y(Alignment::Center);

    if frame.settings {
        // Nothing in the header acts on the file while the settings are up;
        // the only useful control is the one that puts the file back.
        return controls
            .push(action_button(
                "go-previous-symbolic",
                Message::ToggleSettings,
            ))
            .push(action_button("window-close-symbolic", Message::Dismiss))
            .into();
    }

    // Zoom is only meaningful where there is something to scale.
    if frame.preview.is_visual() {
        controls = controls
            .push(action_button("zoom-out-symbolic", Message::Zoom(0.8)))
            .push(action_button("zoom-in-symbolic", Message::Zoom(1.25)));
    }

    if frame.entry.is_some() {
        controls = controls
            .push(action_button("edit-copy-symbolic", Message::CopyPath))
            .push(action_button(
                "view-fullscreen-symbolic",
                Message::ToggleFullscreen,
            ))
            .push(action_button("view-grid-symbolic", Message::ToggleGrid))
            .push(action_button(
                "emblem-system-symbolic",
                Message::ToggleSettings,
            ))
            .push(
                button::text(fl!("open"))
                    .on_press(Message::OpenExternally)
                    .class(cosmic::theme::Button::Suggested),
            );
    }

    controls
        .push(action_button("window-close-symbolic", Message::Dismiss))
        .apply(container)
        // Buttons carry their own theming and cannot be faded the way drawn
        // colours are, so the whole strip is simply hidden until the panel has
        // substantially arrived. Fading a button to 60% would show a
        // half-rendered control rather than a fading one.
        .apply(|element| {
            if alpha > 0.6 {
                Element::from(element)
            } else {
                Element::from(space::horizontal().width(Length::Shrink))
            }
        })
}

fn action_button<'a>(name: &'a str, message: Message) -> Element<'a, Message> {
    button::icon(icon::from_name(name).size(16))
        .on_press(message)
        .class(cosmic::theme::Button::Icon)
        .into()
}

/// The body of the panel, dispatched on what the preview turned out to be.
fn content<'a>(frame: &Frame<'a>, alpha: f32, width: f32, height: f32) -> Element<'a, Message> {
    let (step_alpha, offset) = frame.panel.step(frame.now);
    let alpha = alpha * step_alpha;

    if frame.grid {
        return container(grid_body(frame, alpha, width))
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .padding([0, PANEL_PADDING as u16])
            .into();
    }

    if frame.settings {
        return container(settings_body(frame.config, alpha))
            .width(Length::Fixed(width))
            .height(Length::Fixed(height))
            .padding([0, PANEL_PADDING as u16])
            .into();
    }

    let body: Element<'a, Message> = match frame.preview {
        Preview::Pending => centred(hint(fl!("loading"), alpha)),
        Preview::Picture(_) | Preview::Pdf(_) | Preview::Comic(_) => visual(frame, alpha),
        Preview::Media(media) => {
            if media.has_video || media.poster.is_some() {
                visual(frame, alpha)
            } else {
                media_details(frame, alpha)
            }
        }
        Preview::Text(document) => {
            text_body(document, alpha, frame.config, height, frame.text_scroll)
        }
        Preview::Ebook(book) => ebook_body(book, alpha, frame.image),
        Preview::Font(font) => font_body(font, alpha),
        Preview::Archive(archive) => archive_body(archive, alpha),
        Preview::Directory(directory) => directory_body(directory, alpha),
        Preview::Card(card) => card_body(card, &[], alpha),
        Preview::Failed { reason, card, .. } => {
            let note = reason_text(*reason);
            card_body(card, std::slice::from_ref(&note), alpha)
        }
    };

    container(body)
        .width(Length::Fixed(width))
        .height(Length::Fixed(height))
        // The step slide is expressed as asymmetric padding: shifting the
        // content inside a fixed-size container moves it without resizing the
        // panel, which would otherwise jump on every arrow press.
        .padding(if frame.fullscreen {
            [0, 0, 0, 0]
        } else {
            [
                0,
                (PANEL_PADDING - offset.min(0.0)) as u16,
                0,
                (PANEL_PADDING + offset.max(0.0)) as u16,
            ]
        })
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

/// An image, a PDF page, or a video frame — all the same widget.
///
/// The widget also owns the pointer's half of zooming: scrolling over the
/// content zooms it, dragging pans it once zoom has cropped it, and a double
/// click jumps between the fit and the file's own pixels. Wrapping the content
/// rather than the panel is what keeps a click on the picture from being a
/// click on the backdrop — dismissing the preview by clicking the thing being
/// previewed is nobody's intent.
fn visual<'a>(frame: &Frame<'a>, alpha: f32) -> Element<'a, Message> {
    let Some(handle) = frame.image else {
        return centred(hint(fl!("nothing-to-show"), alpha));
    };

    let widget = image(handle.clone())
        .content_fit(ContentFit::Contain)
        // The one place iced offers a real opacity, so a visual preview fades
        // properly rather than relying on the panel behind it.
        .opacity(alpha);

    let viewport = surface::content_size(frame.screen, frame.preview, frame.config, frame.layout());
    let drawn = surface::image_size(frame.screen, frame.preview, frame.config, frame.layout());

    let sized: Element<'a, Message> = match (viewport, drawn) {
        // Zoom has grown the image past its viewport: draw it at full size
        // inside a clipped window, offset by the pan. The pan arrives already
        // clamped, so the image's edges never leave the frame.
        (Some(viewport), Some(drawn))
            if drawn.width > viewport.width + 0.5 || drawn.height > viewport.height + 0.5 =>
        {
            let offset = Vector::new(
                (viewport.width - drawn.width) / 2.0 + frame.pan.x,
                (viewport.height - drawn.height) / 2.0 + frame.pan.y,
            );

            container(
                pin(widget
                    .width(Length::Fixed(drawn.width))
                    .height(Length::Fixed(drawn.height)))
                .x(offset.x)
                .y(offset.y),
            )
            .width(Length::Fixed(viewport.width))
            .height(Length::Fixed(viewport.height))
            .clip(true)
            .into()
        }

        // Drawn at the size the panel was built around, rather than filling
        // it. The panel has a minimum width, so `Fill` would blow a small
        // image up to reach it — the exact stretching `surface::content_size`
        // exists to prevent.
        (Some(viewport), _) => widget
            .width(Length::Fixed(viewport.width))
            .height(Length::Fixed(viewport.height))
            .into(),

        _ => widget.width(Length::Fill).height(Length::Fill).into(),
    };

    mouse_area(sized)
        .on_scroll(|delta| {
            // Exponential, so equal wheel travel means equal magnification
            // whichever direction it happens in, and the two delta units meet
            // at roughly the same speed.
            let factor = match delta {
                ScrollDelta::Lines { y, .. } => (y * 0.15).exp(),
                ScrollDelta::Pixels { y, .. } => (y * 0.002).exp(),
            };
            Message::Zoom(factor)
        })
        .on_press(Message::DragStart)
        .on_release(Message::DragEnd)
        // A drag that leaves the content must not stay stuck to the pointer
        // when it returns unpressed.
        .on_exit(Message::DragEnd)
        .on_move(Message::PointerMoved)
        .on_double_click(Message::ToggleActualSize)
        .into()
}

/// Audio with no cover art: the tags and the stream description.
fn media_details<'a>(frame: &Frame<'a>, alpha: f32) -> Element<'a, Message> {
    let Preview::Media(media) = frame.preview else {
        return centred(hint(fl!("nothing-to-show"), alpha));
    };

    if !media.decodable {
        return centred(hint(fl!("no-codec"), alpha));
    }

    let mut rows = column::with_capacity(media.tags.len() + media.streams.len())
        .spacing(6)
        .width(Length::Fill);

    for (label, value) in &media.tags {
        rows = rows.push(labelled(label, value, alpha));
    }
    for stream in &media.streams {
        rows = rows.push(labelled(&fl!("stream"), stream, alpha));
    }

    scrollable(rows).height(Length::Fill).into()
}

/// Logical lines rendered beyond each edge of the visible range.
///
/// Enough that ordinary scrolling stays inside the rendered window and the
/// next view — driven by the scroll message that just arrived — re-centres it
/// before the edge is ever seen.
const TEXT_OVERDRAW: usize = 120;

/// A highlighted text document.
///
/// Two layouts behind one function, chosen by [`peek_engine::Document::prose`],
/// because source and prose want opposite typography. **Source** is
/// monospaced, numbered, unwrapped, and *windowed*: only the lines near the
/// scroll position are shaped, with spacers of exactly the right height
/// standing in for the rest, which is what lets a hundred-thousand-line log
/// scroll at all. **Prose** — Markdown, an extracted document — is
/// proportional, wrapped, and unnumbered, and is therefore shaped whole; the
/// renderers that produce it cap their output for that reason.
fn text_body<'a>(
    document: &'a peek_engine::Document,
    alpha: f32,
    config: &Config,
    height: f32,
    scroll: f32,
) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();
    let body = fade(Color::from(cosmic.on_bg_color()), alpha);
    let muted = fade(Color::from(cosmic.on_bg_color()), alpha * MUTED);

    let line_height = surface::CODE_LINE_HEIGHT;
    let total = document.lines.len();

    // Prose is not windowed: wrapping makes a line's drawn height unknown, and
    // the spacer arithmetic windowing depends on needs it exactly.
    let (first, last) = if document.prose {
        (0, total)
    } else {
        let first =
            ((scroll / line_height).floor().max(0.0) as usize).saturating_sub(TEXT_OVERDRAW);
        let visible = (height / line_height).ceil() as usize + 1;
        (first, (first + visible + TEXT_OVERDRAW * 2).min(total))
    };

    let face = if document.prose {
        cosmic::font::default()
    } else {
        cosmic::font::mono()
    };

    // One span list for the whole window, newlines included — see the module
    // comment. `2` per line covers the average run count without reallocating.
    // The span type is generic over a link payload; nothing here is clickable,
    // so it is pinned to the unit type rather than left to inference.
    let mut spans: Vec<cosmic::iced::advanced::text::Span<'a, ()>> =
        Vec::with_capacity((last - first) * 2);
    for (index, line) in document.lines[first..last].iter().enumerate() {
        if index > 0 {
            spans.push(span("\n"));
        }
        for run in &line.spans {
            // An all-zero colour is the highlighter's way of saying "no opinion"
            // — see `peek_engine::text::plain` — and has to follow the desktop
            // theme rather than render as literal black.
            let colour = if run.color == [0, 0, 0] {
                body
            } else {
                fade(
                    Color::from_rgb8(run.color[0], run.color[1], run.color[2]),
                    alpha,
                )
            };
            // Weight and slant are what carry a heading or an emphasis once
            // the Markdown punctuation that meant them has been rendered away.
            spans.push(
                span(run.text.as_str())
                    .color(colour)
                    .font(styled_face(face, run.bold, run.italic)),
            );
        }
    }

    let mut code = rich_text(spans).size(CODE_SIZE).font(face);
    if document.prose {
        // Prose wraps; the reading column exists to give it a measure.
        code = code.size(PROSE_SIZE);
    } else {
        // The explicit line height is what makes the spacer arithmetic exact;
        // the default would track the font and drift the scrollbar. Wrapping
        // is off for the same reason — a wrapped line would be two lines tall
        // and every number after it would sit beside the wrong line.
        code = code
            .line_height(LineHeight::Absolute(line_height.into()))
            .wrapping(Wrapping::None);
    }
    let code = code.apply(container).width(Length::Fill);

    let mut body_row = row::with_capacity(2).spacing(12);

    // Numbers are for source. Prose has paragraphs, not lines, and numbering
    // them would be numbering the wrapping.
    if config.line_numbers && !document.prose {
        // A parallel column of numbers in the same monospace face at the same
        // size and line height, so the two line up without either knowing the
        // other's metrics.
        let numbers = (first + 1..=last)
            .map(|number| format!("{number}"))
            .collect::<Vec<_>>()
            .join("\n");

        body_row = body_row.push(
            text::body(numbers)
                .size(CODE_SIZE)
                .line_height(LineHeight::Absolute(line_height.into()))
                .font(cosmic::font::mono())
                .align_x(Alignment::End)
                .class(colour(muted)),
        );
    }

    // Zero spacing: the two spacers and the window must add up to exactly
    // `total * line_height`, and column spacing would be added between them.
    let mut stack = column::with_capacity(4).spacing(0).width(Length::Fill);

    if first > 0 {
        stack = stack.push(space::vertical().height(Length::Fixed(first as f32 * line_height)));
    }
    stack = stack.push(body_row.push(code));
    if last < total {
        stack = stack
            .push(space::vertical().height(Length::Fixed((total - last) as f32 * line_height)));
    }

    if document.truncated || document.lossy {
        let mut notes = Vec::new();
        if document.truncated {
            notes.push(fl!("truncated-file"));
        }
        if document.lossy {
            notes.push(fl!("lossy-file"));
        }
        stack = stack.push(hint(notes.join(" \u{b7} "), alpha));
    }

    scrollable(stack)
        .on_scroll(|viewport| Message::TextScrolled(viewport.absolute_offset().y))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// A face carrying a span's weight and slant.
fn styled_face(base: cosmic::iced::Font, bold: bool, italic: bool) -> cosmic::iced::Font {
    use cosmic::iced::font::{Style, Weight};

    cosmic::iced::Font {
        weight: if bold { Weight::Bold } else { base.weight },
        style: if italic { Style::Italic } else { base.style },
        ..base
    }
}

/// The settings panel.
///
/// Inside the overlay rather than in a libcosmic context drawer: a context
/// drawer belongs to an application window with a header bar, and this is a
/// layer surface that deliberately has neither. The nine keys were previously
/// reachable only by writing RON into dotfiles, which is not a setting a user
/// can be expected to find.
fn settings_body<'a>(config: &Config, alpha: f32) -> Element<'a, Message> {
    use crate::config::Autoplay;
    use cosmic::widget::{settings, toggler};

    let toggle = |label: String, value: bool, message: fn(bool) -> Setting| {
        settings::item(
            label,
            toggler(value).on_toggle(move |value| Message::Setting(message(value))),
        )
    };

    // Three modes shown at once rather than behind a dropdown: they fit, and
    // which one is active is the thing the user came here to check.
    let current = config.autoplay;
    let mode = move |label: String, value: Autoplay| {
        button::text(label)
            .on_press(Message::Setting(Setting::Autoplay(value)))
            .class(if current == value {
                cosmic::theme::Button::Suggested
            } else {
                cosmic::theme::Button::Standard
            })
    };

    let appearance = settings::section()
        .title(fl!("settings-appearance"))
        .add(toggle(fl!("setting-blur"), config.blur, Setting::Blur))
        .add(toggle(
            fl!("setting-animate"),
            config.animate,
            Setting::Animate,
        ))
        .add(settings::item(
            fl!("setting-opacity"),
            slider(0.4..=1.0, config.panel_opacity(), |value| {
                Message::Setting(Setting::Opacity(value))
            })
            .step(0.02_f32)
            .width(Length::Fixed(SLIDER_WIDTH)),
        ))
        .add(settings::item(
            fl!("setting-max-fraction"),
            slider(0.4..=0.98, config.fraction(), |value| {
                Message::Setting(Setting::MaxFraction(value))
            })
            .step(0.02_f32)
            .width(Length::Fixed(SLIDER_WIDTH)),
        ));

    let content = settings::section()
        .title(fl!("settings-content"))
        .add(toggle(
            fl!("setting-line-numbers"),
            config.line_numbers,
            Setting::LineNumbers,
        ))
        .add(settings::item(
            fl!("setting-autoplay"),
            row::with_capacity(3)
                .push(mode(fl!("autoplay-always"), Autoplay::Always))
                .push(mode(fl!("autoplay-video-only"), Autoplay::VideoOnly))
                .push(mode(fl!("autoplay-never"), Autoplay::Never))
                .spacing(4),
        ))
        .add(toggle(
            fl!("setting-loop"),
            config.loop_media,
            Setting::LoopMedia,
        ));

    let behaviour = settings::section()
        .title(fl!("settings-behaviour"))
        .add(toggle(
            fl!("setting-follow-selection"),
            config.follow_selection,
            Setting::FollowSelection,
        ))
        .add(toggle(
            fl!("setting-click-away"),
            config.click_away,
            Setting::ClickAway,
        ));

    let _ = alpha;
    scrollable(
        column::with_capacity(3)
            .push(appearance)
            .push(content)
            .push(behaviour)
            .spacing(16)
            .width(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// The index sheet: every file in the selection, at once.
///
/// QuickLook's other surface. Arrow navigation shows one file and asks the
/// user to remember the rest; the grid shows the rest. Cells are the shared
/// thumbnail renderer's output — the same one the file manager's icons come
/// from — falling back to the file's type icon where there is no picture to
/// make, which is most of what a directory of source files looks like.
fn grid_body<'a>(frame: &Frame<'a>, alpha: f32, width: f32) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();
    let body = fade(Color::from(cosmic.on_bg_color()), alpha);
    let accent = fade(Color::from(cosmic.accent_color()), alpha);

    let paths = frame.around.paths();
    if paths.is_empty() {
        return centred(hint(fl!("nothing-to-show"), alpha));
    }

    // Columns from the width available rather than a constant, so the sheet
    // reflows with the panel. Shared with the app, which moves the cursor by
    // this same count when the up arrow is pressed.
    let columns = surface::grid_columns(width + PANEL_PADDING * 2.0);

    let mut sheet = column::with_capacity(paths.len().div_ceil(columns))
        .spacing(CELL_SPACING)
        .width(Length::Fill);

    for (row_index, chunk) in paths.chunks(columns).enumerate() {
        let mut row_widget = row::with_capacity(columns).spacing(CELL_SPACING);

        for (column_index, path) in chunk.iter().enumerate() {
            let index = row_index * columns + column_index;
            row_widget = row_widget.push(cell(frame, path, index, body, accent, alpha));
        }

        // The last row is padded so its cells keep the others' width rather
        // than stretching to share the row between them.
        for _ in chunk.len()..columns {
            row_widget = row_widget.push(space::horizontal().width(Length::Fixed(CELL_EDGE)));
        }

        sheet = sheet.push(row_widget);
    }

    scrollable(sheet)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// One cell of the index sheet.
fn cell<'a>(
    frame: &Frame<'a>,
    path: &'a std::path::Path,
    index: usize,
    body: Color,
    accent: Color,
    alpha: f32,
) -> Element<'a, Message> {
    let selected = index == frame.grid_index;

    // A present `None` is a file with no thumbnail and an absent key is one
    // still rendering; both draw the type icon, so they flatten to one arm.
    let picture: Element<'a, Message> = match frame.thumbnails.get(path).and_then(Option::as_ref) {
        Some(handle) => image(handle.clone())
            .content_fit(ContentFit::Contain)
            .opacity(alpha)
            .width(Length::Fixed(CELL_EDGE))
            .height(Length::Fixed(THUMBNAIL_EDGE))
            .into(),
        // No thumbnail: the file's own type icon, which is what the file
        // manager shows for it and is more informative than a blank cell.
        None => {
            let mime = path
                .file_name()
                .map(|_| peek_engine::kind::detect(path).0)
                .unwrap_or_default();
            icon::from_name(meta::icon_name(&mime))
                .fallback(Some(icon::IconFallback::Names(vec![
                    meta::generic_icon_name(&mime).into(),
                ])))
                .size(64)
                .icon()
                .apply(container)
                .width(Length::Fixed(CELL_EDGE))
                .height(Length::Fixed(THUMBNAIL_EDGE))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .into()
        }
    };

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let contents = column::with_capacity(2)
        .push(picture)
        .push(
            text::caption(name)
                .wrapping(Wrapping::None)
                .align_x(Alignment::Center)
                .class(colour(if selected { accent } else { body }))
                .width(Length::Fixed(CELL_EDGE)),
        )
        .spacing(4)
        .align_x(Alignment::Center);

    let framed = container(contents)
        .padding(6)
        .clip(true)
        .class(cosmic::theme::Container::custom(move |_| {
            cosmic::iced::widget::container::Style {
                border: cosmic::iced::Border {
                    radius: 8.0.into(),
                    // The cursor is drawn as an outline rather than a fill so
                    // it does not tint the thumbnail it surrounds.
                    width: if selected { 2.0 } else { 0.0 },
                    color: accent,
                },
                ..Default::default()
            }
        }));

    mouse_area(framed).on_press(Message::GridPick(index)).into()
}

/// An EPUB: its cover beside the opening pages./// An EPUB: its cover beside the opening pages.
///
/// The cover leads because it is how a reader recognises a book, and the text
/// follows because the cover alone does not answer "is this the edition I
/// wanted". Both scroll together — a cover pinned above scrolling text would
/// waste the panel's height on a book with no cover at all.
fn ebook_body<'a>(
    book: &'a peek_engine::Ebook,
    alpha: f32,
    cover: Option<&image::Handle>,
) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();
    let body = fade(Color::from(cosmic.on_bg_color()), alpha);

    let mut stack = column::with_capacity(book.lines.len() + 4)
        .spacing(8)
        .width(Length::Fill);

    if let Some(cover) = cover {
        stack = stack.push(
            image(cover.clone())
                .content_fit(ContentFit::Contain)
                .opacity(alpha)
                .height(Length::Fixed(COVER_HEIGHT))
                .apply(container)
                .width(Length::Fill)
                .align_x(Alignment::Center)
                .padding([0, 0, 8, 0]),
        );
    }

    if let Some(title) = &book.title {
        stack = stack.push(text::title3(title.clone()).class(colour(body)));
    }
    if !book.authors.is_empty() {
        stack = stack.push(hint(book.authors.join(", "), alpha));
    }

    for line in &book.lines {
        stack = stack.push(text::body(line.plain()).class(colour(body)));
    }

    if book.truncated {
        stack = stack.push(hint(fl!("truncated-book"), alpha));
    }

    scrollable(stack)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// A font specimen: the pangram at descending sizes, then the facts.
///
/// The specimen is drawn in the font itself — the engine hands over the file's
/// bytes and the frontend's text stack registers them, which is the one thing
/// it does better than any rasteriser this crate could carry. Where
/// registration has not happened the sizes still read as a specimen in the
/// interface font, which is a weaker preview but not a broken one.
fn font_body<'a>(font: &'a peek_engine::FontSpecimen, alpha: f32) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();
    let body = fade(Color::from(cosmic.on_bg_color()), alpha);

    let face = crate::fonts::face_for(font);

    let mut stack = column::with_capacity(SPECIMEN_SIZES.len() + 6)
        .spacing(10)
        .width(Length::Fill);

    if !font.family.is_empty() {
        stack = stack.push(
            text::body(font.family.clone())
                .size(28)
                .font(face)
                .class(colour(body)),
        );
    }

    let pangram = fl!("pangram");
    for size in SPECIMEN_SIZES {
        stack = stack.push(
            text::body(pangram.clone())
                .size(*size)
                .font(face)
                .wrapping(Wrapping::None)
                .class(colour(body)),
        );
    }

    // The alphabet and digits, which is what someone judging a face for code
    // or for a heading actually looks at.
    stack = stack.push(
        text::body(fl!("specimen-alphabet"))
            .size(18)
            .font(face)
            .class(colour(body)),
    );

    stack = stack.push(labelled(
        &fl!("field-glyphs"),
        &fl!("glyph-count", count = plural_count(u64::from(font.glyphs))),
        alpha,
    ));
    if let Some(version) = &font.version {
        stack = stack.push(labelled(&fl!("field-version"), version, alpha));
    }
    if font.faces > 1 {
        stack = stack.push(labelled(
            &fl!("field-faces"),
            &font.faces.to_string(),
            alpha,
        ));
    }

    let mut traits = Vec::new();
    if font.variable {
        traits.push(fl!("font-variable"));
    }
    if font.monospaced {
        traits.push(fl!("font-monospaced"));
    }
    if !traits.is_empty() {
        stack = stack.push(hint(traits.join(" · "), alpha));
    }

    scrollable(stack)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// An archive's contents.
fn archive_body<'a>(archive: &'a peek_engine::Archive, alpha: f32) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();
    let body = fade(Color::from(cosmic.on_bg_color()), alpha);
    let muted = fade(Color::from(cosmic.on_bg_color()), alpha * MUTED);

    if archive.members.is_empty() {
        return centred(hint(fl!("empty-archive"), alpha));
    }

    let mut list = column::with_capacity(archive.members.len() + 2)
        .spacing(2)
        .width(Length::Fill);

    for member in &archive.members {
        let leading = icon::from_name(if member.is_dir {
            "folder-symbolic"
        } else {
            "text-x-generic-symbolic"
        })
        .size(14)
        .icon();

        let mut trailing = if member.is_dir {
            String::new()
        } else {
            meta::size(member.size)
        };
        if member.encrypted {
            trailing.push_str(&format!(" · {}", fl!("locked-entry")));
        }

        list = list.push(
            row::with_capacity(4)
                .push(leading)
                .push(text::caption(member.name.as_str()).class(colour(body)))
                .push(space::horizontal())
                .push(text::caption(trailing).class(colour(muted)))
                .spacing(8)
                .align_y(Alignment::Center),
        );
    }

    let mut footer = fl!("uncompressed-total", size = meta::size(archive.total_size));
    if archive.truncated {
        footer.push_str(&format!(" · {}", fl!("more-entries")));
    }
    list = list.push(hint(footer, alpha));

    scrollable(list)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// A directory's contents.
fn directory_body<'a>(directory: &'a peek_engine::Directory, alpha: f32) -> Element<'a, Message> {
    if directory.unreadable {
        return centred(hint(fl!("unreadable-folder"), alpha));
    }

    let mut list = column::with_capacity(directory.sample.len() + 3)
        .spacing(4)
        .width(Length::Fill);

    list = list
        .push(labelled(
            &fl!("contents"),
            &folder_summary(directory),
            alpha,
        ))
        .push(labelled(&fl!("size"), &meta::size(directory.size), alpha));

    for name in &directory.sample {
        list = list.push(
            row::with_capacity(2)
                .push(icon::from_name("folder-open-symbolic").size(14).icon())
                .push(text::caption(name.as_str()))
                .spacing(8)
                .align_y(Alignment::Center),
        );
    }

    if directory.truncated {
        list = list.push(hint(fl!("partial-folder"), alpha));
    }

    scrollable(list)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The metadata card, optionally preceded by why a previewer declined.
fn card_body<'a>(
    card: &'a peek_engine::Card,
    notes: &[String],
    alpha: f32,
) -> Element<'a, Message> {
    let mut stack = column::with_capacity(card.rows.len() + notes.len() + 1)
        .spacing(8)
        .width(Length::Fill);

    for note in notes {
        stack = stack.push(hint(note.clone(), alpha));
    }

    stack = stack.push(
        icon::from_name(card.icon.as_str())
            .fallback(Some(icon::IconFallback::Names(vec![
                "application-x-generic".into(),
            ])))
            .size(64)
            .icon()
            .apply(container)
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .padding([8, 0]),
    );

    for (field, value) in &card.rows {
        stack = stack.push(labelled(&field_label(*field), &field_value(value), alpha));
    }

    scrollable(stack)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// The transport strip under a playable file.
fn transport<'a>(frame: &Frame<'a>, alpha: f32) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();
    let muted = fade(Color::from(cosmic.on_bg_color()), alpha * MUTED);

    let playing = frame.player.is_some_and(Player::is_playing);
    let position = frame.player.and_then(Player::position).unwrap_or_default();
    let duration = frame
        .player
        .and_then(Player::duration)
        .or(match frame.preview {
            Preview::Media(media) => media.duration,
            _ => None,
        })
        .unwrap_or_default();

    // Guarded rather than clamped: a stream whose duration is unknown reports
    // zero, and dividing by it would put the handle at NaN.
    let progress = if duration.is_zero() {
        0.0
    } else {
        (position.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
    };

    row::with_capacity(4)
        .push(action_button(
            if playing {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            },
            Message::TogglePlay,
        ))
        .push(
            slider(0.0..=1.0, progress, Message::SeekTo)
                .step(0.001_f32)
                .width(Length::Fill),
        )
        .push(
            text::caption(fl!(
                "transport-position",
                position = meta::duration(position),
                duration = meta::duration(duration)
            ))
            .font(cosmic::font::mono())
            .class(colour(muted)),
        )
        .spacing(12)
        .align_y(Alignment::Center)
        .apply(container)
        .padding([0, PANEL_PADDING as u16])
        .height(Length::Fixed(TRANSPORT_HEIGHT))
        .width(Length::Fill)
        .into()
}

/// A label/value pair, the unit every detail panel is built from.
fn labelled<'a>(label: &str, value: &str, alpha: f32) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();
    let body = fade(Color::from(cosmic.on_bg_color()), alpha);
    let muted = fade(Color::from(cosmic.on_bg_color()), alpha * MUTED);

    row::with_capacity(2)
        .push(
            text::caption(label.to_owned())
                .class(colour(muted))
                .width(Length::Fixed(120.0)),
        )
        .push(text::caption(value.to_owned()).class(colour(body)))
        .spacing(12)
        .align_y(Alignment::Start)
        .into()
}

/// Secondary text: reasons, limits, and empty states.
fn hint<'a>(message: impl Into<String>, alpha: f32) -> Element<'a, Message> {
    let theme = cosmic::theme::active();
    let cosmic = theme.cosmic();
    let muted = fade(Color::from(cosmic.on_bg_color()), alpha * MUTED);

    text::caption(message.into()).class(colour(muted)).into()
}

/// Centre a single element in whatever space it is given.
fn centred<'a>(element: Element<'a, Message>) -> Element<'a, Message> {
    container(element)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

/// A text style that is just a colour.
///
/// The faded colours this module computes have to be applied per widget, and
/// spelling out the closure at every call site is most of the noise it would
/// otherwise carry.
fn colour(color: Color) -> cosmic::theme::Text {
    cosmic::theme::Text::Color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip Fluent's bidi isolation marks.
    ///
    /// Fluent wraps every interpolated value in U+2068/U+2069 so a right-to-left
    /// value cannot reorder the text around it. They render as nothing and every
    /// COSMIC application leaves them in place; they are only in the way of an
    /// exact comparison.
    fn plain_text(value: String) -> String {
        value.replace(['\u{2068}', '\u{2069}'], "")
    }

    /// Fluent selects `[one]` from the *number*, not from the text, so a count
    /// handed over as a string silently reads "1 minutes ago". These pin the
    /// selection rather than the wording.
    #[test]
    fn counts_of_one_use_the_singular() {
        crate::localize::localize();

        let render = |elapsed| plain_text(field_value(&meta::Value::Elapsed(elapsed)));

        assert_eq!(render(meta::Elapsed::Minutes(1)), "1 minute ago");
        assert_eq!(render(meta::Elapsed::Hours(1)), "1 hour ago");
        assert_eq!(render(meta::Elapsed::Days(1)), "1 day ago");
    }

    #[test]
    fn other_counts_use_the_plural() {
        crate::localize::localize();

        let render = |elapsed| plain_text(field_value(&meta::Value::Elapsed(elapsed)));

        assert_eq!(render(meta::Elapsed::Minutes(2)), "2 minutes ago");
        assert_eq!(render(meta::Elapsed::Hours(0)), "0 hours ago");
        assert_eq!(render(meta::Elapsed::JustNow), "just now");
    }

    #[test]
    fn mime_types_get_readable_names_in_the_users_language() {
        crate::localize::localize();

        assert_eq!(plain_text(mime_name("image/png")), "PNG image");
        assert_eq!(plain_text(mime_name("application/pdf")), "PDF document");
        assert_eq!(plain_text(mime_name("image/x-tga")), "TGA image");
        assert_eq!(plain_text(mime_name("audio/flac")), "FLAC audio");
        // The fallback keeps the subtype rather than saying "Unknown".
        assert_eq!(
            plain_text(mime_name("application/x-frobnicator")),
            "FROBNICATOR file"
        );
    }

    #[test]
    fn every_failure_reason_has_words() {
        crate::localize::localize();

        for reason in [
            Reason::Unreadable,
            Reason::Undecodable,
            Reason::Encrypted,
            Reason::TooLarge,
            Reason::Empty,
            Reason::NoCodec,
            Reason::NoPreview,
        ] {
            let text = reason_text(reason);
            // A missing catalogue entry renders as the message id, which is
            // kebab-case and would ship as a visible label.
            assert!(!text.contains("reason-"), "unresolved text for {reason:?}");
            assert!(!text.is_empty());
        }
    }

    #[test]
    fn every_field_has_a_label() {
        crate::localize::localize();

        for field in [
            meta::Field::Kind,
            meta::Field::Type,
            meta::Field::Size,
            meta::Field::Modified,
            meta::Field::Permissions,
            meta::Field::SymlinkTo,
            meta::Field::Where,
        ] {
            let label = field_label(field);
            // A missing catalogue entry renders as the message id, which is
            // kebab-case and would otherwise ship as a visible label.
            assert!(!label.contains('-'), "unresolved label for {field:?}");
            assert!(!label.is_empty());
        }
    }

    #[test]
    fn fading_scales_alpha_and_leaves_the_colour_alone() {
        let faded = fade(Color::from_rgba(0.2, 0.4, 0.6, 0.8), 0.5);
        assert!((faded.a - 0.4).abs() < f32::EPSILON);
        assert!((faded.r - 0.2).abs() < f32::EPSILON);
    }

    #[test]
    fn fading_is_clamped_at_both_ends() {
        let colour = Color::from_rgba(1.0, 1.0, 1.0, 1.0);
        assert_eq!(fade(colour, 2.0).a, 1.0);
        assert_eq!(fade(colour, -1.0).a, 0.0);
    }

    #[test]
    fn only_previews_with_pixels_produce_a_handle() {
        assert!(handle_for(&Preview::Pending).is_none());
        assert!(handle_for(&Preview::Card(Box::default())).is_none());
    }

    #[test]
    fn a_raster_becomes_a_handle_of_the_same_size() {
        let raster = Raster::new(2, 2, vec![0; 16]).expect("raster");
        let handle = handle_for_raster(&raster);
        // The handle keeps the dimensions it was built with; a mismatch here
        // would show as a torn image rather than as an error.
        match handle {
            image::Handle::Rgba { width, height, .. } => {
                assert_eq!((width, height), (2, 2));
            }
            _ => panic!("expected an rgba handle"),
        }
    }
}
