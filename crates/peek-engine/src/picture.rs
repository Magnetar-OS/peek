// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Raster and vector image previews.
//!
//! Two rules shape this module.
//!
//! **Decode to the size that will be shown, not to the size on disk.** A 60
//! megapixel photo is 240 MB of RGBA, and uploading that to the GPU to draw it
//! at 1200 px wide costs a visible pause for no visible benefit. Formats whose
//! decoders can subsample do so; the rest are decoded and immediately reduced.
//!
//! **Vectors are rendered, never scaled.** An SVG rasterised at thumbnail size
//! and then stretched to fill the screen is the one failure a vector previewer
//! is judged on, so [`vector`] takes the target size as an argument rather than
//! producing a fixed bitmap.

use std::path::Path;
use std::time::Duration;

use image::imageops::FilterType;
use image::metadata::Orientation;
// `orientation` and `from_decoder` live on the decoder trait, which has to be
// in scope even though the decoder itself is an opaque `impl`.
use image::{AnimationDecoder, ImageDecoder, ImageFormat, ImageReader};

use crate::raster::Raster;

/// Longest edge, in pixels, that a decoded image is reduced to.
///
/// Sized for a 4K display with room to zoom in a little before the softness
/// shows. Above this the extra pixels are never sampled: the preview surface is
/// at most the size of the output.
pub const MAX_EDGE: u32 = 4096;

/// Refuse to decode images beyond this many pixels.
///
/// Not a memory limit — a limit on *time*. A deliberately-crafted 65535×65535
/// PNG decodes for minutes, and a previewer that hangs on one file in a
/// directory is worse than one that declines to show it.
const MAX_PIXELS: u64 = 512 * 1024 * 1024;

/// Frames decoded from an animated image before it is cut short.
///
/// Every decoded frame is held for as long as the file is on screen, so the cap
/// is what keeps a ten-minute GIF from being decoded whole into memory before
/// its first frame appears.
const MAX_FRAMES: usize = 600;

/// Bytes of decoded frames an animation may occupy.
///
/// A frame cap alone is not enough: six hundred frames of a 4K screen recording
/// is nine gigabytes of RGBA. Whichever limit is hit first ends the decode, and
/// the animation reports itself truncated.
const MAX_ANIMATION_BYTES: usize = 256 * 1024 * 1024;

/// One frame of an animated image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimationFrame {
    pub raster: Raster,
    /// How long the frame stays on screen before the next one.
    pub delay: Duration,
}

/// A decoded animation: every frame, fully composited.
///
/// Frames arrive composited rather than as deltas — the `image` crate applies
/// each format's disposal rules — so the frontend can show any frame on its own
/// without replaying the ones before it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Animation {
    /// Always at least two: a single-frame "animation" is a still image and is
    /// treated as one.
    pub frames: Vec<AnimationFrame>,
    /// Set when the file held more frames than the decode budget allowed.
    pub truncated: bool,
}

/// What a decoded image carries besides its pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Picture {
    pub raster: Raster,
    /// Dimensions as stored, before any reduction. The overlay shows these,
    /// because "3024 × 4032" is a fact about the file and the reduced size is
    /// an implementation detail.
    pub source_width: u32,
    pub source_height: u32,
    /// Camera, lens, and exposure, when the file carries EXIF.
    pub exif: Vec<(String, String)>,
    /// Whether the source can be re-rendered at any size.
    ///
    /// True for vectors, false for rasters. It is what stops a 16×16 icon being
    /// blown up to fill the panel: a raster has a real, finite resolution and
    /// showing it larger than that shows the scaler, not the file.
    pub scalable: bool,
    /// The remaining frames, when the file is animated. [`Picture::raster`] is
    /// the first frame either way, so a frontend that ignores this field still
    /// shows the image — just still.
    pub animation: Option<Animation>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("could not decode: {0}")]
    Decode(#[from] image::ImageError),
    #[error("could not parse the SVG: {0}")]
    Svg(#[from] resvg::usvg::Error),
    #[error("image is {width}×{height}, which is too large to decode")]
    TooLarge { width: u32, height: u32 },
    #[error("the rendered image was empty")]
    Empty,
}

/// Decode a raster image, reducing it to something a display can use.
///
/// # Errors
///
/// Fails when the file cannot be read, cannot be decoded by any of the enabled
/// codecs, or is large enough that decoding it would stall the previewer.
pub fn raster(path: &Path) -> Result<Picture, Error> {
    let reader = ImageReader::open(path)
        .map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?
        // The extension may be wrong — detection is content-first everywhere
        // else, so it has to be here too.
        .with_guessed_format()
        .map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?;

    let format = reader.format();
    let (source_width, source_height) = reader.into_dimensions()?;
    if u64::from(source_width) * u64::from(source_height) > MAX_PIXELS {
        return Err(Error::TooLarge {
            width: source_width,
            height: source_height,
        });
    }

    // Animated formats are tried as animations first. A GIF that turns out to
    // hold one frame falls through to the still path below, so nothing is
    // decoded twice for the common case of a static file in an animated
    // container.
    if let Some(animation) = format.and_then(|format| animation(path, format)) {
        let first = animation.frames[0].raster.clone();
        return Ok(Picture {
            raster: first,
            source_width,
            source_height,
            // Animated containers do not carry camera EXIF worth showing.
            exif: Vec::new(),
            scalable: false,
            animation: Some(animation),
        });
    }

    // Reopened rather than kept: `into_dimensions` consumes the reader, and
    // reading a header twice is cheaper than buffering the whole file to avoid
    // it.
    let reader = ImageReader::open(path)
        .map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?
        .with_guessed_format()
        .map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?;

    let mut decoder = reader.into_decoder()?;
    // Read before decoding: `orientation` consumes the metadata the decoder
    // holds, and JPEGs from every phone in existence rely on it. Without this
    // portrait photos preview on their side.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);

    let mut decoded = image::DynamicImage::from_decoder(decoder)?;
    decoded.apply_orientation(orientation);

    // Orientation can transpose the image, so the displayed dimensions are the
    // ones after the transform, not the ones in the header.
    let (source_width, source_height) = (decoded.width(), decoded.height());

    let reduced = reduce(decoded, MAX_EDGE);
    let raster =
        Raster::new(reduced.width(), reduced.height(), reduced.into_raw()).ok_or(Error::Empty)?;

    Ok(Picture {
        raster,
        source_width,
        source_height,
        exif: exif_summary(path),
        scalable: false,
        animation: None,
    })
}

/// Decode the frames of an animated image, if the file actually animates.
///
/// Returns `None` for a format that cannot animate, a file with a single
/// frame, and any decode failure — in all three cases the still path is the
/// right answer, so there is nothing to report.
fn animation(path: &Path, format: ImageFormat) -> Option<Animation> {
    let open = || std::fs::File::open(path).ok().map(std::io::BufReader::new);

    let frames = match format {
        ImageFormat::Gif => image::codecs::gif::GifDecoder::new(open()?)
            .ok()?
            .into_frames(),
        ImageFormat::Png => {
            let decoder = image::codecs::png::PngDecoder::new(open()?).ok()?;
            if !decoder.is_apng().ok()? {
                return None;
            }
            decoder.apng().ok()?.into_frames()
        }
        ImageFormat::WebP => {
            let decoder = image::codecs::webp::WebPDecoder::new(open()?).ok()?;
            if !decoder.has_animation() {
                return None;
            }
            decoder.into_frames()
        }
        _ => return None,
    };

    let mut out: Vec<AnimationFrame> = Vec::new();
    let mut bytes = 0usize;
    let mut truncated = false;

    for frame in frames {
        let Ok(frame) = frame else {
            // A damaged tail. The frames already decoded still animate, which
            // is more of the file than refusing to would show.
            break;
        };

        let delay = normalise_delay(Duration::from(frame.delay()));
        let reduced = reduce(
            image::DynamicImage::ImageRgba8(frame.into_buffer()),
            MAX_EDGE,
        );
        bytes += reduced.as_raw().len();

        let Some(raster) = Raster::new(reduced.width(), reduced.height(), reduced.into_raw())
        else {
            break;
        };
        out.push(AnimationFrame { raster, delay });

        if out.len() >= MAX_FRAMES || bytes >= MAX_ANIMATION_BYTES {
            truncated = true;
            break;
        }
    }

    (out.len() > 1).then_some(Animation {
        frames: out,
        truncated,
    })
}

/// Bring a frame delay into the range players actually honour.
///
/// GIFs in the wild carry zero and near-zero delays that were never meant
/// literally; every browser shows those at 100 ms, and matching that is what
/// makes the same file animate here at the speed it does everywhere else.
fn normalise_delay(delay: Duration) -> Duration {
    if delay <= Duration::from_millis(10) {
        Duration::from_millis(100)
    } else {
        delay
    }
}

/// Render an SVG at a target size.
///
/// `target` is the longest edge the preview will occupy. The document's own
/// aspect ratio is preserved; only the scale comes from the caller.
///
/// # Errors
///
/// Fails when the file cannot be read or is not a parseable SVG.
pub fn vector(path: &Path, target: u32) -> Result<Picture, Error> {
    use resvg::tiny_skia::{Pixmap, Transform};
    use resvg::usvg;

    let data = std::fs::read(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })?;

    let mut options = usvg::Options::default();
    // Text in an SVG is laid out against real fonts; without the system
    // database every labelled diagram renders as a diagram with no labels.
    options.fontdb_mut().load_system_fonts();
    // Relative `href`s resolve against the file's own directory, which is what
    // a browser would do and what the author expected.
    if let Some(parent) = path.parent() {
        options.resources_dir = Some(parent.to_path_buf());
    }

    let tree = usvg::Tree::from_data(&data, &options)?;
    let size = tree.size();

    let (source_width, source_height) = (size.width().ceil() as u32, size.height().ceil() as u32);
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return Err(Error::Empty);
    }

    // Scale so the longest edge lands on the target. Vectors are also scaled
    // *up* here, unlike rasters — that is the entire point of having them.
    let scale = (target as f32 / size.width().max(size.height())).max(0.01);
    let width = ((size.width() * scale).round() as u32).clamp(1, MAX_EDGE);
    let height = ((size.height() * scale).round() as u32).clamp(1, MAX_EDGE);

    let mut pixmap = Pixmap::new(width, height).ok_or(Error::Empty)?;
    resvg::render(
        &tree,
        Transform::from_scale(width as f32 / size.width(), height as f32 / size.height()),
        &mut pixmap.as_mut(),
    );

    // tiny-skia works in premultiplied alpha; `take` hands back the raw buffer,
    // so it has to be converted the same way cairo's is.
    let pixels = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            [
                pixel.demultiply().red(),
                pixel.demultiply().green(),
                pixel.demultiply().blue(),
                pixel.alpha(),
            ]
        })
        .collect();

    Ok(Picture {
        raster: Raster::new(width, height, pixels).ok_or(Error::Empty)?,
        source_width,
        source_height,
        exif: Vec::new(),
        scalable: true,
        animation: None,
    })
}

/// Reduce an image so its longest edge is at most `edge`.
///
/// Lanczos3 rather than a cheaper filter: this runs once per file, and the
/// result is what the user judges the previewer on. Triangle at a 10× reduction
/// visibly aliases fine detail, which is exactly what someone previewing a photo
/// is looking at.
///
/// Public because embedded cover art arrives as bytes rather than as a path and
/// still has to be bounded the same way — see [`crate::media::cover_art`].
#[must_use]
pub fn reduce(image: image::DynamicImage, edge: u32) -> image::RgbaImage {
    let (width, height) = (image.width(), image.height());
    if width <= edge && height <= edge {
        return image.into_rgba8();
    }

    let scale = f64::from(edge) / f64::from(width.max(height));
    let target_width = ((f64::from(width) * scale).round() as u32).max(1);
    let target_height = ((f64::from(height) * scale).round() as u32).max(1);

    image
        .resize(target_width, target_height, FilterType::Lanczos3)
        .into_rgba8()
}

/// EXIF fields worth showing, in the order a photographer reads them.
///
/// A deliberately short list. Dumping every tag turns the panel into a hex
/// viewer, and the fields that answer "what was this shot with" are the same six
/// every time.
const EXIF_FIELDS: &[(exif::Tag, &str)] = &[
    (exif::Tag::Make, "Camera"),
    (exif::Tag::Model, "Model"),
    (exif::Tag::LensModel, "Lens"),
    (exif::Tag::FNumber, "Aperture"),
    (exif::Tag::ExposureTime, "Shutter"),
    (exif::Tag::PhotographicSensitivity, "ISO"),
    (exif::Tag::FocalLength, "Focal length"),
    (exif::Tag::DateTimeOriginal, "Taken"),
];

/// Read the interesting EXIF fields, if there are any.
///
/// Every failure here is silent and returns an empty list: a photo without EXIF
/// is normal, and a corrupt EXIF block is not a reason to refuse to show the
/// picture.
///
/// `pub(crate)` because camera raw files are TIFF containers the same reader
/// parses, and the raw previewer shows the same six fields.
pub(crate) fn exif_summary(path: &Path) -> Vec<(String, String)> {
    let Ok(file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let mut reader = std::io::BufReader::new(file);
    let Ok(exif) = exif::Reader::new().read_from_container(&mut reader) else {
        return Vec::new();
    };

    EXIF_FIELDS
        .iter()
        .filter_map(|(tag, label)| {
            let field = exif.get_field(*tag, exif::In::PRIMARY)?;
            let value = field.display_value().with_unit(&exif).to_string();
            (!value.is_empty()).then(|| ((*label).to_owned(), value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_images_are_left_alone() {
        let image = image::DynamicImage::new_rgba8(100, 80);
        let reduced = reduce(image, MAX_EDGE);
        assert_eq!((reduced.width(), reduced.height()), (100, 80));
    }

    #[test]
    fn large_images_are_reduced_preserving_aspect() {
        let image = image::DynamicImage::new_rgba8(8000, 4000);
        let reduced = reduce(image, 4000);
        assert_eq!((reduced.width(), reduced.height()), (4000, 2000));
    }

    #[test]
    fn reduction_never_produces_a_zero_edge() {
        // An extreme panorama would round its short edge to zero without the
        // clamp, and a zero-height texture is a GPU error rather than an image.
        let image = image::DynamicImage::new_rgba8(20000, 3);
        let reduced = reduce(image, 1024);
        assert!(reduced.height() >= 1);
    }

    #[test]
    fn zero_delays_are_normalised_the_way_browsers_do() {
        assert_eq!(
            normalise_delay(Duration::ZERO),
            Duration::from_millis(100),
            "a zero delay was never meant literally"
        );
        assert_eq!(
            normalise_delay(Duration::from_millis(40)),
            Duration::from_millis(40)
        );
    }

    #[test]
    fn an_animated_gif_decodes_every_frame() {
        use image::codecs::gif::GifEncoder;
        use image::{Delay, Frame, Rgba, RgbaImage};

        let path = std::env::temp_dir().join("peek-test-animated.gif");
        {
            let file = std::fs::File::create(&path).expect("create");
            let mut encoder = GifEncoder::new(file);
            encoder
                .set_repeat(image::codecs::gif::Repeat::Infinite)
                .expect("repeat");
            for shade in [0u8, 128, 255] {
                let frame = Frame::from_parts(
                    RgbaImage::from_pixel(4, 4, Rgba([shade, 0, 0, 255])),
                    0,
                    0,
                    Delay::from_numer_denom_ms(200, 1),
                );
                encoder.encode_frame(frame).expect("encode");
            }
        }

        let picture = raster(&path).expect("decodes");
        let animation = picture.animation.expect("an animation");
        assert_eq!(animation.frames.len(), 3);
        assert!(!animation.truncated);
        assert_eq!(animation.frames[0].delay, Duration::from_millis(200));
        // The first frame is also the still raster, so a frontend that ignores
        // animation entirely still shows the file.
        assert_eq!(picture.raster, animation.frames[0].raster);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_single_frame_gif_is_a_still_image() {
        use image::codecs::gif::GifEncoder;
        use image::{Frame, Rgba, RgbaImage};

        let path = std::env::temp_dir().join("peek-test-still.gif");
        {
            let file = std::fs::File::create(&path).expect("create");
            let mut encoder = GifEncoder::new(file);
            encoder
                .encode_frame(Frame::new(RgbaImage::from_pixel(
                    4,
                    4,
                    Rgba([0, 255, 0, 255]),
                )))
                .expect("encode");
        }

        let picture = raster(&path).expect("decodes");
        assert!(
            picture.animation.is_none(),
            "one frame is a still, not an animation"
        );

        let _ = std::fs::remove_file(path);
    }
}
