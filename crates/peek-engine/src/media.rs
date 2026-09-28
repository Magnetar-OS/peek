// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Audio and video previews.
//!
//! Two separate jobs, deliberately kept apart.
//!
//! [`probe`] answers "what is this file" — duration, resolution, codecs, tags,
//! and a poster frame — without ever starting playback. It is what the overlay
//! shows in the instant between the file being selected and the user deciding
//! whether to press play, and it is the *only* thing that runs for a file the
//! user is merely arrowing past.
//!
//! [`Player`] runs a real pipeline and pushes decoded frames. It is created only
//! when playback actually starts, because a `playbin` per file in a directory
//! being scrolled through would have a dozen decoders alive at once.
//!
//! GStreamer is the right tool here for the same reason `light` reuses
//! `pop-launcher`: every codec on the machine is already registered with it, and
//! a previewer that shipped its own decoders would support fewer formats than
//! the video player the user already has.

use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
// The `Discoverer*Ext` and `VideoFrameExt` traits carry almost every accessor
// used below; without them the types look like they have no methods at all.
use gstreamer_pbutils::prelude::*;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::VideoFrameExt;

use crate::raster::Raster;

/// How far into a video the poster frame is taken from.
///
/// Not frame zero: the first frame of a video is very often black, a fade-in, or
/// a title card, and a poster that is a black rectangle tells the user nothing
/// about the file. A fixed fraction rather than a fixed time so it lands
/// sensibly on both a six-second clip and a two-hour film.
const POSTER_POSITION: f64 = 0.15;

/// How long to wait for a pipeline to preroll before giving up on a poster.
///
/// A network mount or an exotic codec can take arbitrarily long, and the
/// metadata is worth showing without it.
const PREROLL_TIMEOUT: gst::ClockTime = gst::ClockTime::from_seconds(5);

/// Longest edge a poster frame is scaled to.
const POSTER_EDGE: u32 = 1600;

/// Longest edge a playback frame is handed over at.
///
/// The largest texture edge a GPU is guaranteed to accept (wgpu's default
/// `max_texture_dimension_2d`). 8K video fits; anything larger is scaled to
/// it in the pipeline rather than dropped at the upload.
const FRAME_EDGE: u32 = 8192;

/// What a media file says about itself.
#[derive(Debug, Clone, Default)]
pub struct Media {
    pub duration: Option<Duration>,
    /// Video dimensions, when there is a video stream.
    pub width: u32,
    pub height: u32,
    pub has_video: bool,
    pub has_audio: bool,
    /// One line per stream: codec, and the details that distinguish it.
    pub streams: Vec<String>,
    /// Track tags, for audio. Empty for most video.
    pub tags: Vec<(String, String)>,
    /// Poster frame for video, embedded cover art for audio.
    pub poster: Option<Raster>,
    /// Whether GStreamer could decode the file at all. A file that probes but
    /// reports no streams is one the desktop has no codec for, and saying so is
    /// more useful than showing an empty panel.
    pub decodable: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GStreamer could not start: {0}")]
    Init(String),
    #[error("could not build the pipeline: {0}")]
    Pipeline(String),
    #[error("could not read {0}")]
    Uri(String),
}

/// Initialise GStreamer exactly once.
///
/// Registry construction is the expensive part and it is process-wide, so this
/// is behind a `OnceLock` rather than being called per preview.
fn init() -> Result<(), Error> {
    static READY: OnceLock<Result<(), String>> = OnceLock::new();
    READY
        .get_or_init(|| gst::init().map_err(|error| error.to_string()))
        .clone()
        .map_err(Error::Init)
}

fn uri_for(path: &Path) -> Result<String, Error> {
    gst::glib::filename_to_uri(path, None).map_or_else(
        |_| Err(Error::Uri(path.display().to_string())),
        |uri| Ok(uri.to_string()),
    )
}

/// Describe a media file without playing it.
///
/// # Errors
///
/// Fails when GStreamer cannot start or the path cannot be expressed as a URI.
/// A file GStreamer cannot decode is *not* an error: it comes back with
/// `decodable: false`, because "your system has no codec for this" is
/// information the previewer should show rather than swallow.
pub fn probe(path: &Path) -> Result<Media, Error> {
    init()?;
    let uri = uri_for(path)?;

    let mut media = Media {
        tags: audio_tags(path),
        ..Media::default()
    };

    let discoverer = gstreamer_pbutils::Discoverer::new(PREROLL_TIMEOUT)
        .map_err(|error| Error::Pipeline(error.to_string()))?;

    let Ok(info) = discoverer.discover_uri(&uri) else {
        // Discovery failing outright still leaves the tags read above, so the
        // preview is a track listing rather than nothing at all.
        return Ok(media);
    };

    media.duration = info
        .duration()
        .map(|clock| Duration::from_nanos(clock.nseconds()));

    // The stream the poster is taken from: the largest, as it is displayed.
    let mut poster_from: Option<(u32, u32, gst::Fraction)> = None;

    for stream in info.video_streams() {
        media.has_video = true;
        media.width = media.width.max(stream.width());
        media.height = media.height.max(stream.height());
        if poster_from.is_none_or(|(width, height, _)| {
            u64::from(stream.width()) * u64::from(stream.height())
                > u64::from(width) * u64::from(height)
        }) {
            poster_from = Some((stream.width(), stream.height(), stream.par()));
        }

        let mut description = codec_name(stream.upcast_ref());
        if stream.width() > 0 {
            description.push_str(&format!(" · {}×{}", stream.width(), stream.height()));
        }
        let framerate = stream.framerate();
        if framerate.denom() > 0 && framerate.numer() > 0 {
            let fps = f64::from(framerate.numer()) / f64::from(framerate.denom());
            description.push_str(&format!(" · {fps:.3} fps"));
        }
        media.streams.push(description);
    }

    for stream in info.audio_streams() {
        media.has_audio = true;

        let mut description = codec_name(stream.upcast_ref());
        if stream.channels() > 0 {
            description.push_str(&format!(" · {} ch", stream.channels()));
        }
        if stream.sample_rate() > 0 {
            description.push_str(&format!(" · {} Hz", stream.sample_rate()));
        }
        if stream.bitrate() > 0 {
            description.push_str(&format!(" · {} kbps", stream.bitrate() / 1000));
        }
        media.streams.push(description);
    }

    media.decodable = media.has_video || media.has_audio;

    // Cover art from the tags is preferred over a decoded frame for audio; for
    // video there is no cover, so a frame is the only option.
    if media.poster.is_none()
        && let Some((width, height, par)) = poster_from
        && let Some(size) = poster_size(width, height, (par.numer(), par.denom()))
    {
        media.poster = poster(&uri, media.duration, size);
    }

    Ok(media)
}

/// Human-readable codec name for a stream.
///
/// `pb_utils` produces the same strings the desktop's media player shows, which
/// is the point — the previewer should not invent its own vocabulary for
/// something the user can look up elsewhere.
fn codec_name(stream: &gstreamer_pbutils::DiscovererStreamInfo) -> String {
    stream
        .caps()
        .map(|caps| gstreamer_pbutils::functions::pb_utils_get_codec_description(&caps).to_string())
        .unwrap_or_else(|| "Unknown codec".to_owned())
}

/// Which edge of a poster is fixed, and to what: the displayed frame's
/// longer edge, reduced to [`POSTER_EDGE`] and never enlarged.
///
/// Only one edge is given to the pipeline. `videoscale` derives the other
/// from the display aspect ratio, so an anamorphic stream is neither
/// stretched nor letterboxed — which fixing both edges to rounded numbers
/// would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PosterSize {
    Width(u32),
    Height(u32),
}

/// The poster size for a video stream of `width` × `height` stored pixels
/// with pixel aspect ratio `par` (numerator, denominator).
fn poster_size(width: u32, height: u32, par: (i32, i32)) -> Option<PosterSize> {
    if width == 0 || height == 0 {
        return None;
    }
    // Stored pixels are not always square: a DVD's 720 stored columns are
    // shown 853 wide. The poster is judged by the frame as it is displayed.
    let displayed_width = match par {
        (numerator, denominator) if numerator > 0 && denominator > 0 => {
            u64::from(width) * u64::from(numerator.unsigned_abs())
                / u64::from(denominator.unsigned_abs())
        }
        _ => u64::from(width),
    }
    .max(1);

    let edge = u64::from(POSTER_EDGE);
    Some(if displayed_width >= u64::from(height) {
        PosterSize::Width(u32::try_from(displayed_width.min(edge)).unwrap_or(POSTER_EDGE))
    } else {
        PosterSize::Height(height.min(POSTER_EDGE))
    })
}

/// Decode a single frame to use as a poster.
///
/// Runs the pipeline only as far as `Paused`, which decodes exactly one frame —
/// the preroll — and stops. Returns `None` on any failure: a video with no
/// poster still previews as a video, so nothing here is worth surfacing as an
/// error.
fn poster(uri: &str, duration: Option<Duration>, size: PosterSize) -> Option<Raster> {
    // `videoconvert` handles whatever the decoder produces; `videoscale`
    // brings it down to the poster's size, so a 4K frame does not become a
    // 33 MB texture for a thumbnail and an 8K one still gets a poster.
    let edge = match size {
        PosterSize::Width(width) => format!("width={width}"),
        PosterSize::Height(height) => format!("height={height}"),
    };
    let description = format!(
        "uridecodebin uri=\"{uri}\" ! videoconvert ! videoscale ! \
         video/x-raw,format=RGBA,pixel-aspect-ratio=1/1,{edge} ! \
         appsink name=poster max-buffers=1 drop=false sync=false"
    );

    let pipeline = gst::parse::launch(&description)
        .ok()?
        .downcast::<gst::Pipeline>()
        .ok()?;

    let sink = pipeline
        .by_name("poster")?
        .downcast::<gst_app::AppSink>()
        .ok()?;

    if pipeline.set_state(gst::State::Paused).is_err() {
        return None;
    }

    // Wait for the preroll rather than assuming the state change is synchronous;
    // for anything but a local raw file it is not.
    let prerolled = pipeline.state(PREROLL_TIMEOUT).0.is_ok();
    if !prerolled {
        let _ = pipeline.set_state(gst::State::Null);
        return None;
    }

    // Seek past the opening. A failed seek is not fatal — the preroll frame is
    // still a frame, just a less representative one.
    if let Some(duration) = duration
        && duration > Duration::from_secs(1)
    {
        let target = duration.mul_f64(POSTER_POSITION);
        let _ = pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
            gst::ClockTime::from_nseconds(target.as_nanos() as u64),
        );
        let _ = pipeline.state(PREROLL_TIMEOUT);
    }

    let sample = sink.pull_preroll().ok();
    let raster = sample
        .as_ref()
        .and_then(|sample| sample_to_raster(sample, POSTER_EDGE));

    let _ = pipeline.set_state(gst::State::Null);
    raster
}

/// Convert a GStreamer RGBA sample into a [`Raster`].
///
/// The stride is the reason this is not a memcpy: GStreamer aligns each row,
/// so a 1023-pixel-wide frame has padding at the end of every line that must
/// not be copied into a tightly-packed buffer.
///
/// `max_edge` bounds the result's longer edge; frames larger than it are
/// dropped rather than scaled, because the pipeline is where scaling belongs
/// and doing it twice would soften the image for nothing.
fn sample_to_raster(sample: &gst::Sample, max_edge: u32) -> Option<Raster> {
    let caps = sample.caps()?;
    let info = gst_video::VideoInfo::from_caps(caps).ok()?;
    let buffer = sample.buffer()?;
    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).ok()?;

    let width = frame.width();
    let height = frame.height();
    if width == 0 || height == 0 || width.max(height) > max_edge {
        return None;
    }

    let stride = frame.plane_stride()[0].max(0) as usize;
    let data = frame.plane_data(0).ok()?;
    let row_bytes = width as usize * 4;

    let mut pixels = Vec::with_capacity(row_bytes * height as usize);
    for row in 0..height as usize {
        let start = row * stride;
        let end = start + row_bytes;
        if end > data.len() {
            return None;
        }
        pixels.extend_from_slice(&data[start..end]);
    }

    Raster::new(width, height, pixels)
}

/// Read audio tags and cover art with lofty.
///
/// Pure Rust and far cheaper than starting a pipeline, and it reads the
/// embedded picture that GStreamer's discoverer only exposes as a tag sample.
fn audio_tags(path: &Path) -> Vec<(String, String)> {
    use lofty::file::TaggedFileExt;
    use lofty::prelude::{Accessor, ItemKey};

    let Ok(tagged) = lofty::read_from_path(path) else {
        return Vec::new();
    };
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    let mut push = |label: &str, value: Option<String>| {
        if let Some(value) = value.filter(|text| !text.trim().is_empty()) {
            rows.push((label.to_owned(), value));
        }
    };

    push("Title", tag.title().map(Into::into));
    push("Artist", tag.artist().map(Into::into));
    push("Album", tag.album().map(Into::into));
    // `Accessor` covers the fields every format shares; a release year is
    // spelled differently in each container, so it comes through `ItemKey`.
    push("Year", tag.get_string(ItemKey::Year).map(str::to_owned));
    push("Track", tag.track().map(|track| track.to_string()));
    push("Genre", tag.genre().map(Into::into));
    push(
        "Composer",
        tag.get_string(ItemKey::Composer).map(str::to_owned),
    );

    rows
}

/// Embedded cover art, decoded.
///
/// Separate from [`audio_tags`] because it is the one field that costs an image
/// decode, and a file the user is arrowing past does not need it until it is
/// actually on screen.
#[must_use]
pub fn cover_art(path: &Path) -> Option<Raster> {
    use lofty::file::TaggedFileExt;

    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let picture = tag.pictures().first()?;

    let decoded = image::load_from_memory(picture.data()).ok()?;
    let reduced = crate::picture::reduce(decoded, POSTER_EDGE);
    Raster::new(reduced.width(), reduced.height(), reduced.into_raw())
}

/// A running playback pipeline.
///
/// Owns a `playbin`, which is GStreamer's "play this URI" element: it picks
/// decoders, wires up an audio sink, and handles seeking. The video sink is
/// replaced with an `appsink` so frames come back as buffers this process can
/// draw rather than going to a window the compositor would have to place
/// somewhere.
pub struct Player {
    pipeline: gst::Pipeline,
    sink: gst_app::AppSink,
    /// Whether the pipeline has a video stream at all. Audio-only playback
    /// still runs through `playbin`, it just never produces a frame.
    has_video: bool,
}

impl std::fmt::Debug for Player {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Player")
            .field("has_video", &self.has_video)
            .field("position", &self.position())
            .finish()
    }
}

impl Player {
    /// Build a pipeline for a file, paused and prerolled.
    ///
    /// Paused rather than playing: the overlay decides when playback starts, and
    /// a preview that begins blaring audio the moment the selection moves is the
    /// single most annoying thing a previewer can do.
    ///
    /// # Errors
    ///
    /// Fails when GStreamer cannot start, the path cannot be expressed as a
    /// URI, or `playbin` is not installed.
    pub fn open(path: &Path, has_video: bool) -> Result<Self, Error> {
        init()?;
        let uri = uri_for(path)?;

        let pipeline = gst::ElementFactory::make("playbin3")
            .build()
            // playbin3 is the current element but is not present on every
            // installation; the older playbin has the same interface for
            // everything used here.
            .or_else(|_| gst::ElementFactory::make("playbin").build())
            .map_err(|error| Error::Pipeline(error.to_string()))?
            .downcast::<gst::Pipeline>()
            .map_err(|_| Error::Pipeline("playbin is not a pipeline".to_owned()))?;

        pipeline.set_property("uri", &uri);

        // A size *range* rather than a size: `videoscale` passes a frame that
        // fits through untouched and brings one that does not down to fit,
        // keeping its display aspect ratio.
        let edge = gst::IntRange::new(1, i32::try_from(FRAME_EDGE).unwrap_or(i32::MAX));
        let sink = gst_app::AppSink::builder()
            .caps(
                &gst::Caps::builder("video/x-raw")
                    .field("format", "RGBA")
                    .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
                    .field("width", edge)
                    .field("height", edge)
                    .build(),
            )
            // One buffer, dropping the rest. The overlay draws at display rate
            // and pulls the newest frame; queueing frames it will never show
            // would only add latency and memory.
            .max_buffers(1)
            .drop(true)
            .build();

        // `videoconvert` in front of the sink because a decoder is free to
        // produce any format it likes, and the caps above would otherwise fail
        // to link rather than being negotiated into; `videoscale` for the
        // same reason about size.
        let bin = gst::Bin::builder().name("peek-video-sink").build();
        let convert = gst::ElementFactory::make("videoconvert")
            .build()
            .map_err(|error| Error::Pipeline(error.to_string()))?;
        let scale = gst::ElementFactory::make("videoscale")
            .build()
            .map_err(|error| Error::Pipeline(error.to_string()))?;

        bin.add_many([&convert, &scale, sink.upcast_ref::<gst::Element>()])
            .map_err(|error| Error::Pipeline(error.to_string()))?;
        gst::Element::link_many([&convert, &scale, sink.upcast_ref::<gst::Element>()])
            .map_err(|error| Error::Pipeline(error.to_string()))?;

        let pad = convert
            .static_pad("sink")
            .ok_or_else(|| Error::Pipeline("videoconvert has no sink pad".to_owned()))?;
        let ghost =
            gst::GhostPad::with_target(&pad).map_err(|error| Error::Pipeline(error.to_string()))?;
        bin.add_pad(&ghost)
            .map_err(|error| Error::Pipeline(error.to_string()))?;

        pipeline.set_property("video-sink", &bin);

        pipeline
            .set_state(gst::State::Paused)
            .map_err(|error| Error::Pipeline(error.to_string()))?;

        Ok(Self {
            pipeline,
            sink,
            has_video,
        })
    }

    /// Start or resume playback.
    pub fn play(&self) {
        let _ = self.pipeline.set_state(gst::State::Playing);
    }

    /// Pause, leaving the pipeline prerolled so resuming is instant.
    pub fn pause(&self) {
        let _ = self.pipeline.set_state(gst::State::Paused);
    }

    /// Whether the pipeline is rolling, or about to be.
    ///
    /// The pending state counts, and that is not a detail. `set_state` is
    /// asynchronous: it returns `Async` and the pipeline reaches `Playing` some
    /// milliseconds later. A caller that asks `current_state()` immediately
    /// after starting playback is told "no" — and a frontend that decides
    /// whether to ask for frames from that answer stops asking, which means
    /// nothing ever drives the pipeline forward and playback silently never
    /// appears to start.
    #[must_use]
    pub fn is_playing(&self) -> bool {
        // Zero timeout: this is polled from the draw path and must not block on
        // a state change that is still in flight.
        let (_, current, pending) = self.pipeline.state(gst::ClockTime::ZERO);
        current == gst::State::Playing || pending == gst::State::Playing
    }

    /// Jump to a position.
    ///
    /// `Accurate` rather than `KeyUnit`: a preview scrub bar that lands on the
    /// nearest keyframe can be several seconds away from where the user
    /// pointed, which reads as the control being broken.
    pub fn seek(&self, position: Duration) {
        let _ = self.pipeline.seek_simple(
            gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
            gst::ClockTime::from_nseconds(position.as_nanos() as u64),
        );
    }

    /// Current playback position.
    #[must_use]
    pub fn position(&self) -> Option<Duration> {
        self.pipeline
            .query_position::<gst::ClockTime>()
            .map(|clock| Duration::from_nanos(clock.nseconds()))
    }

    /// Total duration, as the pipeline reports it.
    ///
    /// Preferred over the value from [`probe`] while playing: for a stream whose
    /// container understates its length, the pipeline's answer improves as it
    /// decodes.
    #[must_use]
    pub fn duration(&self) -> Option<Duration> {
        self.pipeline
            .query_duration::<gst::ClockTime>()
            .map(|clock| Duration::from_nanos(clock.nseconds()))
    }

    /// The most recent decoded frame, if a new one is ready.
    ///
    /// Non-blocking and returns `None` when nothing new has arrived, so the
    /// caller can poll it once per display frame and keep showing the previous
    /// image in between.
    #[must_use]
    pub fn frame(&self) -> Option<Raster> {
        if !self.has_video {
            return None;
        }
        // `try_pull_sample` with a zero timeout: this is called from the draw
        // path, so blocking here would stall the whole overlay behind the
        // decoder.
        let sample = self.sink.try_pull_sample(gst::ClockTime::ZERO)?;
        sample_to_raster(&sample, FRAME_EDGE)
    }

    /// Whether playback has reached the end of the file.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        let Some(bus) = self.pipeline.bus() else {
            return false;
        };
        // Draining rather than peeking: an error message left on the bus would
        // otherwise be seen again on every poll.
        while let Some(message) = bus.pop() {
            match message.view() {
                gst::MessageView::Eos(_) => return true,
                gst::MessageView::Error(error) => {
                    tracing::warn!(error = %error.error(), "playback failed");
                    return true;
                }
                _ => {}
            }
        }
        false
    }
}

impl Drop for Player {
    /// Tear the pipeline down explicitly.
    ///
    /// A `playbin` left in `Playing` holds the audio device and its decoder
    /// threads until the process exits — and this process is a daemon that does
    /// not exit between previews.
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_media_file_probes_as_undecodable_rather_than_failing() {
        let path = std::env::temp_dir().join("peek-test-media.bin");
        std::fs::write(&path, b"definitely not a video").expect("write");

        // GStreamer may be unavailable in a build container; that is an Init
        // error and not what this test is about.
        if let Ok(media) = probe(&path) {
            assert!(!media.decodable);
            assert!(media.poster.is_none());
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_poster_fits_its_longer_displayed_edge_and_is_never_enlarged() {
        // 4K and 8K both come down to the poster size; the 8K one used to be
        // rejected outright.
        assert_eq!(
            poster_size(3840, 2160, (1, 1)),
            Some(PosterSize::Width(1600))
        );
        assert_eq!(
            poster_size(7680, 4320, (1, 1)),
            Some(PosterSize::Width(1600))
        );
        // Portrait fixes the height.
        assert_eq!(
            poster_size(1080, 1920, (1, 1)),
            Some(PosterSize::Height(1600))
        );
        // Small video keeps its own size.
        assert_eq!(poster_size(640, 480, (1, 1)), Some(PosterSize::Width(640)));
        // Anamorphic: 720 stored columns shown at 16:9 are 853 wide.
        assert_eq!(
            poster_size(720, 480, (32, 27)),
            Some(PosterSize::Width(853))
        );
        // A PAR the container left unset is read as square.
        assert_eq!(poster_size(640, 480, (0, 1)), Some(PosterSize::Width(640)));
        assert_eq!(poster_size(0, 480, (1, 1)), None);
    }

    #[test]
    fn an_8k_frame_is_handed_over_for_playback() {
        init().expect("GStreamer starts");
        let info = gst_video::VideoInfo::builder(gst_video::VideoFormat::Rgba, 7680, 2)
            .build()
            .expect("video info");
        let buffer = gst::Buffer::from_mut_slice(vec![0u8; info.size()]);
        let caps = info.to_caps().expect("caps");
        let sample = gst::Sample::builder().buffer(&buffer).caps(&caps).build();

        let raster = sample_to_raster(&sample, FRAME_EDGE).expect("an 8K frame fits a texture");
        assert_eq!((raster.width, raster.height), (7680, 2));
    }

    #[test]
    fn probing_a_missing_file_does_not_panic() {
        let _ = probe(Path::new("/nonexistent/peek/clip.mp4"));
    }
}
