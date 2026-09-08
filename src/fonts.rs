// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Registering a previewed font with the text renderer.
//!
//! A font specimen has to be *set in the font*, which means the file's faces
//! must reach the renderer's font system before the panel draws. iced loads
//! fonts through a task, and addresses them afterwards by family name — a
//! `&'static str`, because a font family in an interface is normally a
//! compile-time constant.
//!
//! Here it is not: the family comes off the file the user just pressed space
//! on. So the name is interned, deliberately and permanently. That is not a
//! leak worth avoiding — `iced::font::load` already hands the *font data*, far
//! larger, to a font system that never unloads it, so the name is the smaller
//! half of a cost the load itself decides. What the interning buys is that
//! previewing the same font twice in a session registers it once.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use cosmic::iced::Font;
use peek_engine::FontSpecimen;

/// Family names already handed to the renderer, interned.
fn registry() -> &'static Mutex<HashMap<String, &'static str>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Hand a specimen's faces to the renderer, if they are not already there.
///
/// Returns the task that performs the load, or `None` when the family has been
/// registered before — or when the file names no family, in which case there
/// is nothing to address it by and the specimen falls back to the interface
/// font.
#[must_use]
pub fn register(specimen: &FontSpecimen) -> Option<cosmic::app::Task<crate::app::Message>> {
    if specimen.family.is_empty() {
        return None;
    }

    let Ok(mut registry) = registry().lock() else {
        return None;
    };
    if registry.contains_key(&specimen.family) {
        return None;
    }

    // Interned here rather than at the point of use: `Font::with_name` needs
    // the name to outlive every frame that draws with it, and a preview's
    // lifetime is not that.
    let name: &'static str = String::leak(specimen.family.clone());
    registry.insert(specimen.family.clone(), name);

    // Cloned because the font system takes ownership and the specimen keeps
    // its copy for a re-register after a theme reload.
    Some(
        cosmic::iced::font::load(specimen.data.clone())
            .map(|_| cosmic::action::app(crate::app::Message::None)),
    )
}

/// The font to set a specimen in.
///
/// The registered family when the load has happened, and the interface font
/// until it has. A specimen briefly drawn in the interface font is a weaker
/// preview for one frame; a specimen that refuses to draw is a broken one.
#[must_use]
pub fn face_for(specimen: &FontSpecimen) -> Font {
    registry()
        .lock()
        .ok()
        .and_then(|registry| registry.get(&specimen.family).copied())
        .map_or_else(cosmic::font::default, Font::with_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn specimen(family: &str) -> FontSpecimen {
        FontSpecimen {
            family: family.to_owned(),
            style: None,
            version: None,
            glyphs: 0,
            variable: false,
            monospaced: false,
            faces: 1,
            data: Vec::new(),
        }
    }

    #[test]
    fn a_family_is_registered_once() {
        let font = specimen("Peek Test Family");
        assert!(register(&font).is_some(), "the first load is issued");
        assert!(
            register(&font).is_none(),
            "the second must not reload the same family"
        );
    }

    #[test]
    fn a_nameless_font_is_not_registered() {
        assert!(register(&specimen("")).is_none());
    }

    #[test]
    fn an_unregistered_family_falls_back_to_the_interface_font() {
        let unknown = specimen("Never Registered Peek Face");
        assert_eq!(face_for(&unknown), cosmic::font::default());
    }
}
