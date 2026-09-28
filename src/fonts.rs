// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Registering a previewed font with the text renderer.
//!
//! A font specimen has to be *set in the font*, which means the file's faces
//! must reach the renderer's font system before the panel draws. iced loads
//! fonts through a task, and addresses them afterwards by family name — a
//! `&'static str`, because a font family in an interface is normally a
//! compile-time constant — plus the weight, width and style that pick one face
//! of the family.
//!
//! Here the family is not a constant: it comes off the file the user just
//! pressed space on. So the name is interned, deliberately and permanently.
//! That is not a leak worth avoiding — `iced::font::load` already hands the
//! *font data*, far larger, to a font system that never unloads it, so the
//! name is the smaller half of a cost the load itself decides.
//!
//! Faces are registered and addressed by family *and* weight, width and
//! style. Keyed by family alone, previewing `Inter-Bold.ttf` after
//! `Inter-Regular.ttf` skipped the second load and drew the bold specimen in
//! whichever Inter face the renderer found first.
//!
//! A font whose family is the interface's own is never registered. The
//! overlay's text resolves through that family, and adding the previewed
//! file's faces to it would let a specimen restyle the previewer itself for
//! the rest of the session. Its specimen is drawn in the installed family at
//! the file's weight, width and style, which is the same design.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

use cosmic::iced::Font;
use cosmic::iced::font::{Family, Stretch, Style, Weight};
use peek_engine::FontSpecimen;

/// One face, as the renderer selects it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Face {
    family: String,
    weight: Weight,
    stretch: Stretch,
    style: Style,
}

impl Face {
    fn of(specimen: &FontSpecimen) -> Self {
        Self {
            family: specimen.family.clone(),
            weight: weight(specimen.weight),
            stretch: stretch(specimen.stretch),
            style: if specimen.italic {
                Style::Italic
            } else {
                Style::Normal
            },
        }
    }
}

/// What has been handed to the renderer.
#[derive(Default)]
struct Registry {
    /// Family names, interned once each.
    names: HashMap<String, &'static str>,
    /// Faces whose data has been loaded.
    faces: HashSet<Face>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

/// Hand a specimen's faces to the renderer, if they are not already there.
///
/// Returns the task that performs the load, or `None` when this face has been
/// registered before, when the file names no family (there is nothing to
/// address it by, and the specimen falls back to the interface font), or when
/// the family is the interface's own (see the module documentation).
#[must_use]
pub fn register(specimen: &FontSpecimen) -> Option<cosmic::app::Task<crate::app::Message>> {
    if specimen.family.is_empty() || interface_family(&specimen.family).is_some() {
        return None;
    }

    let Ok(mut registry) = registry().lock() else {
        return None;
    };
    let face = Face::of(specimen);
    if registry.faces.contains(&face) {
        return None;
    }

    // Interned here rather than at the point of use: `Font::with_name` needs
    // the name to outlive every frame that draws with it, and a preview's
    // lifetime is not that.
    if !registry.names.contains_key(&face.family) {
        let name: &'static str = String::leak(face.family.clone());
        registry.names.insert(face.family.clone(), name);
    }
    registry.faces.insert(face);

    // Cloned because the font system takes ownership and the specimen keeps
    // its copy for a re-register after a theme reload.
    Some(
        cosmic::iced::font::load(specimen.data.clone())
            .map(|_| cosmic::action::app(crate::app::Message::None)),
    )
}

/// The font to set a specimen in.
///
/// The registered face when the load has happened — or the interface's own
/// family at the file's weight, width and style — and the interface font
/// until then. A specimen briefly drawn in the interface font is a weaker
/// preview for one frame; a specimen that refuses to draw is a broken one.
#[must_use]
pub fn face_for(specimen: &FontSpecimen) -> Font {
    let face = Face::of(specimen);
    let family = interface_family(&specimen.family).or_else(|| {
        let registry = registry().lock().ok()?;
        registry
            .faces
            .contains(&face)
            .then(|| registry.names.get(&face.family).copied().map(Family::Name))
            .flatten()
    });

    match family {
        Some(family) => Font {
            family,
            weight: face.weight,
            stretch: face.stretch,
            style: face.style,
        },
        None => cosmic::font::default(),
    }
}

/// The interface family a name refers to, when it is one of them.
fn interface_family(name: &str) -> Option<Family> {
    [cosmic::font::default(), cosmic::font::mono()]
        .into_iter()
        .map(|font| font.family)
        .find(|family| matches!(family, Family::Name(own) if own.eq_ignore_ascii_case(name)))
}

/// An OpenType weight class, 100–900, as the renderer's nearest weight.
fn weight(class: u16) -> Weight {
    match class.saturating_add(50) / 100 {
        0 | 1 => Weight::Thin,
        2 => Weight::ExtraLight,
        3 => Weight::Light,
        4 => Weight::Normal,
        5 => Weight::Medium,
        6 => Weight::Semibold,
        7 => Weight::Bold,
        8 => Weight::ExtraBold,
        _ => Weight::Black,
    }
}

/// An OpenType width class, 1–9, as the renderer's stretch.
fn stretch(class: u16) -> Stretch {
    match class {
        1 => Stretch::UltraCondensed,
        2 => Stretch::ExtraCondensed,
        3 => Stretch::Condensed,
        4 => Stretch::SemiCondensed,
        6 => Stretch::SemiExpanded,
        7 => Stretch::Expanded,
        8 => Stretch::ExtraExpanded,
        9 => Stretch::UltraExpanded,
        _ => Stretch::Normal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specimen(family: &str, weight: u16, italic: bool) -> FontSpecimen {
        FontSpecimen {
            family: family.to_owned(),
            style: None,
            weight,
            stretch: 5,
            italic,
            version: None,
            glyphs: 0,
            variable: false,
            monospaced: false,
            faces: 1,
            data: Vec::new(),
        }
    }

    #[test]
    fn a_face_is_registered_once() {
        let font = specimen("Peek Test Family", 400, false);
        assert!(register(&font).is_some(), "the first load is issued");
        assert!(
            register(&font).is_none(),
            "the second must not reload the same face"
        );
    }

    #[test]
    fn each_face_of_a_family_is_registered_and_drawn_as_itself() {
        let regular = specimen("Peek Test Faces", 400, false);
        let bold = specimen("Peek Test Faces", 700, false);
        let italic = specimen("Peek Test Faces", 400, true);

        assert!(register(&regular).is_some());
        assert!(
            register(&bold).is_some(),
            "a second face of the same family is loaded too"
        );
        assert!(register(&italic).is_some());

        let drawn = face_for(&bold);
        assert_eq!(drawn.family, Family::Name("Peek Test Faces"));
        assert_eq!(drawn.weight, Weight::Bold);
        assert_eq!(face_for(&italic).style, Style::Italic);
        assert_eq!(face_for(&regular).weight, Weight::Normal);
    }

    #[test]
    fn the_interface_family_is_never_added_to() {
        let Family::Name(own) = cosmic::font::default().family else {
            panic!("the interface font is named");
        };
        let file = specimen(own, 700, false);

        assert!(
            register(&file).is_none(),
            "the overlay's own text must not change with a preview"
        );
        let drawn = face_for(&file);
        assert_eq!(drawn.family, Family::Name(own));
        assert_eq!(drawn.weight, Weight::Bold);
    }

    #[test]
    fn a_nameless_font_is_not_registered() {
        assert!(register(&specimen("", 400, false)).is_none());
    }

    #[test]
    fn an_unregistered_family_falls_back_to_the_interface_font() {
        let unknown = specimen("Never Registered Peek Face", 400, false);
        assert_eq!(face_for(&unknown), cosmic::font::default());
    }

    #[test]
    fn weight_classes_round_to_the_nearest_weight() {
        assert_eq!(weight(100), Weight::Thin);
        assert_eq!(weight(350), Weight::Normal);
        assert_eq!(weight(400), Weight::Normal);
        assert_eq!(weight(600), Weight::Semibold);
        assert_eq!(weight(950), Weight::Black);
        assert_eq!(stretch(5), Stretch::Normal);
        assert_eq!(stretch(3), Stretch::Condensed);
    }
}
