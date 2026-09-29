// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! The keyboard: one table that the key handler reads and `--help` prints.
//!
//! The overlay has no text input to focus, so every key is handled on the
//! global event listener, whatever the pointer happens to be over. The keys are
//! turned into *directions* and *intentions* rather than actions, because what
//! an arrow means depends on what is on screen — a cell in the index sheet, a
//! page in a document, or the next file — and that decision belongs with the
//! state in [`crate::app`], not here.
//!
//! The help text is generated from the same table, so `--help` cannot list a
//! key the handler ignores or miss one it answers.

use std::fmt::Write as _;

use cosmic::iced::keyboard::key::Named;
use cosmic::iced::keyboard::{Key, Modifiers};

use crate::app::{Message, Nav};

/// A key press a binding answers to.
#[derive(Debug, Clone, Copy)]
enum Trigger {
    /// A named key, whatever modifiers are held.
    Named(Named),
    /// A named key with Shift held.
    ShiftNamed(Named),
    /// A character, whatever modifiers are held.
    Character(&'static str),
    /// A character with Ctrl held.
    CtrlCharacter(&'static str),
}

impl Trigger {
    fn matches(self, key: &Key, modifiers: Modifiers) -> bool {
        match (self, key) {
            (Self::Named(wanted), Key::Named(named)) => wanted == *named,
            (Self::ShiftNamed(wanted), Key::Named(named)) => wanted == *named && modifiers.shift(),
            (Self::Character(wanted), Key::Character(character)) => wanted == character.as_str(),
            (Self::CtrlCharacter(wanted), Key::Character(character)) => {
                wanted == character.as_str() && modifiers.control()
            }
            _ => false,
        }
    }
}

/// One thing a key does.
struct Binding {
    /// The presses that do it. Checked in table order, so a binding that
    /// needs a modifier must come before the plain one for the same key.
    triggers: &'static [Trigger],
    /// The keys as `--help` names them.
    keys: &'static str,
    /// What they do, as `--help` says it.
    does: &'static str,
    message: fn() -> Message,
}

/// Every key the overlay answers.
const BINDINGS: &[Binding] = &[
    Binding {
        // Space arrives as a character rather than as a named key. It closes,
        // the way it does in QuickLook: the key that opened the preview is the
        // key that takes it away.
        triggers: &[Trigger::Character(" "), Trigger::Named(Named::Escape)],
        keys: "Space, Escape",
        does: "Back: out of settings, the index sheet or full screen, then close",
        message: || Message::Escape,
    },
    Binding {
        // Shift turns the horizontal arrows into a scrub, so a video can be
        // seeked without giving up arrow-key navigation between files.
        triggers: &[Trigger::ShiftNamed(Named::ArrowLeft)],
        keys: "Shift+Left",
        does: "Seek back in audio and video",
        message: || Message::Seek(-1),
    },
    Binding {
        triggers: &[Trigger::ShiftNamed(Named::ArrowRight)],
        keys: "Shift+Right",
        does: "Seek forward in audio and video",
        message: || Message::Seek(1),
    },
    Binding {
        triggers: &[Trigger::Named(Named::ArrowLeft)],
        keys: "Left",
        does: "Previous file, or cell in the index sheet",
        message: || Message::Navigate(Nav::Left),
    },
    Binding {
        triggers: &[Trigger::Named(Named::ArrowRight)],
        keys: "Right",
        does: "Next file, or cell in the index sheet",
        message: || Message::Navigate(Nav::Right),
    },
    Binding {
        // Vertical arrows turn pages where there are pages to turn, and move
        // between files where there are not — so they do the obvious thing in
        // both cases without a mode.
        triggers: &[
            Trigger::Named(Named::ArrowUp),
            Trigger::Named(Named::PageUp),
        ],
        keys: "Up, Page Up",
        does: "Previous page (or file, where there are no pages), or row up",
        message: || Message::Navigate(Nav::Up),
    },
    Binding {
        triggers: &[
            Trigger::Named(Named::ArrowDown),
            Trigger::Named(Named::PageDown),
        ],
        keys: "Down, Page Down",
        does: "Next page (or file, where there are no pages), or row down",
        message: || Message::Navigate(Nav::Down),
    },
    Binding {
        triggers: &[Trigger::Named(Named::Enter)],
        keys: "Enter",
        does: "Open in the default application, or open the index sheet's cell",
        message: || Message::Activate,
    },
    Binding {
        triggers: &[Trigger::Character("f"), Trigger::Named(Named::F11)],
        keys: "F, F11",
        does: "Full screen",
        message: || Message::ToggleFullscreen,
    },
    Binding {
        triggers: &[Trigger::Character("g")],
        keys: "G",
        does: "Index sheet: every file in the selection at once",
        message: || Message::ToggleGrid,
    },
    Binding {
        triggers: &[Trigger::Character("p"), Trigger::Character("k")],
        keys: "P, K",
        does: "Play and pause",
        message: || Message::TogglePlay,
    },
    Binding {
        triggers: &[Trigger::Character("+"), Trigger::Character("=")],
        keys: "+",
        does: "Zoom in",
        message: || Message::Zoom(1.25),
    },
    Binding {
        triggers: &[Trigger::Character("-"), Trigger::Character("_")],
        keys: "-",
        does: "Zoom out",
        message: || Message::Zoom(0.8),
    },
    Binding {
        triggers: &[Trigger::Character("0")],
        keys: "0",
        does: "Back to the natural fit",
        message: || Message::ResetZoom,
    },
    Binding {
        triggers: &[Trigger::CtrlCharacter("c")],
        keys: "Ctrl+C",
        does: "Copy the file's path",
        message: || Message::CopyPath,
    },
];

/// What a key press asks for, if anything.
pub fn message(key: &Key, modifiers: Modifiers) -> Option<Message> {
    BINDINGS
        .iter()
        .find(|binding| {
            binding
                .triggers
                .iter()
                .any(|trigger| trigger.matches(key, modifiers))
        })
        .map(|binding| (binding.message)())
}

/// The keys section of `--help`, one line per binding.
pub fn help() -> String {
    let width = BINDINGS
        .iter()
        .map(|binding| binding.keys.len())
        .max()
        .unwrap_or(0)
        + 2;
    let mut help = String::new();
    for binding in BINDINGS {
        let _ = writeln!(help, "  {:width$}{}", binding.keys, binding.does);
    }
    help
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(trigger: Trigger) -> (Key, Modifiers) {
        match trigger {
            Trigger::Named(named) => (Key::Named(named), Modifiers::empty()),
            Trigger::ShiftNamed(named) => (Key::Named(named), Modifiers::SHIFT),
            Trigger::Character(character) => (Key::Character(character.into()), Modifiers::empty()),
            Trigger::CtrlCharacter(character) => {
                (Key::Character(character.into()), Modifiers::CTRL)
            }
        }
    }

    #[test]
    fn every_documented_key_reaches_its_own_action() {
        // A binding shadowed by an earlier one would be documented and never
        // happen — Shift+Left listed after Left, for instance.
        for binding in BINDINGS {
            for &trigger in binding.triggers {
                let (key, modifiers) = key(trigger);
                assert_eq!(
                    format!("{:?}", message(&key, modifiers)),
                    format!("{:?}", Some((binding.message)())),
                    "{} ({trigger:?})",
                    binding.keys
                );
            }
        }
    }

    #[test]
    fn the_help_lists_every_key() {
        let help = help();
        for keys in ["F, F11", "G", "Ctrl+C", "Shift+Left", "P, K", "Page Down"] {
            assert!(help.contains(keys), "{keys} is missing from:\n{help}");
        }
        assert_eq!(help.lines().count(), BINDINGS.len());
    }

    #[test]
    fn a_plain_c_copies_nothing() {
        assert!(message(&Key::Character("c".into()), Modifiers::empty()).is_none());
    }
}
