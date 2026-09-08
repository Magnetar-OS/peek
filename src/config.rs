// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Persisted settings, stored through `cosmic-config`.
//!
//! The same store every COSMIC application uses, under
//! `~/.config/cosmic/dev.entro314labs.Peek/v1/`, one RON file per key. Changes
//! arrive over a subscription and are applied without a restart, which matters
//! for a process that stays resident for the whole session.

use cosmic::cosmic_config::{self, CosmicConfigEntry, cosmic_config_derive::CosmicConfigEntry};
use serde::{Deserialize, Serialize};

/// What happens when a media file becomes the preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Autoplay {
    /// Start playing immediately, the way QuickLook does.
    ///
    /// The default because it is what the gesture is *for*: pressing space on a
    /// video to see whether it is the right one, and having to press a second
    /// key before anything happens defeats the point.
    #[default]
    Always,
    /// Video plays, audio waits.
    ///
    /// For anyone who arrows through a music directory: a silent poster frame
    /// is unobtrusive, twelve tracks each starting for half a second is not.
    VideoOnly,
    /// Nothing plays until asked.
    Never,
}

impl Autoplay {
    /// Whether a stream with these characteristics should start on its own.
    #[must_use]
    pub fn applies(self, has_video: bool) -> bool {
        match self {
            Self::Always => true,
            Self::VideoOnly => has_video,
            Self::Never => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, CosmicConfigEntry)]
#[version = 1]
pub struct Config {
    /// Ask the compositor to blur the backdrop behind the panel.
    pub blur: bool,
    /// Fill opacity of the panel, 0.0..=1.0.
    ///
    /// Higher than `light`'s default: a launcher is read against the desktop,
    /// but a preview *is* the content, and a translucent panel puts the user's
    /// wallpaper behind their photograph.
    pub opacity: f32,
    pub autoplay: Autoplay,
    /// Restart media when it reaches the end.
    pub loop_media: bool,
    /// Show line numbers beside text previews.
    pub line_numbers: bool,
    /// Largest fraction of the display the panel may occupy.
    pub max_fraction: f32,
    /// Serve `org.gnome.NautilusPreviewer2`.
    ///
    /// On by default: it is the only standard interface a file manager uses to
    /// ask for a preview, and serving it is what makes the space bar work in
    /// Nautilus. Turn it off to leave the name to GNOME's own previewer.
    pub nautilus_previewer: bool,
    /// Tell the file manager to move its selection when the arrow keys are used
    /// inside the preview.
    ///
    /// Keeps the two in sync, so closing the preview leaves the cursor on the
    /// file that was last shown rather than on the one it started from.
    pub follow_selection: bool,
    /// Hide the preview when the pointer clicks outside the panel.
    pub click_away: bool,
    /// Play the open, close, and step transitions.
    ///
    /// Off means the panel appears and changes without motion, which is what
    /// vestibular disorders and a preference for reduced motion both need.
    /// A user setting rather than a system one because COSMIC does not yet
    /// publish a reduced-motion preference; when it does, this becomes the
    /// opt-out from it rather than the whole answer — the same relationship
    /// `blur` already has with the desktop's frosted setting.
    pub animate: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            blur: true,
            opacity: 0.86,
            autoplay: Autoplay::default(),
            loop_media: false,
            line_numbers: true,
            max_fraction: 0.86,
            nautilus_previewer: true,
            follow_selection: true,
            click_away: true,
            animate: true,
        }
    }
}

impl Config {
    /// A handle to the config store, for writing settings back.
    ///
    /// Separate from [`Config::load`] because most of the application only
    /// reads: the handle is needed exactly where a setting is changed, and
    /// holding one everywhere would suggest anywhere may write.
    #[must_use]
    pub fn handle() -> Option<cosmic_config::Config> {
        cosmic_config::Config::new(crate::app::APP_ID, Self::VERSION).ok()
    }

    /// Persist the current settings.
    ///
    /// Writes the whole entry rather than one key: the settings view changes
    /// one field at a time, and a whole-entry write is what the derive
    /// guarantees. A failure is logged rather than surfaced — the change is
    /// already live in memory, so the preview is correct either way and only
    /// the persistence is lost.
    pub fn store(&self) {
        let Some(handle) = Self::handle() else {
            tracing::warn!("cosmic-config unavailable; settings will not persist");
            return;
        };
        if let Err(error) = self.write_entry(&handle) {
            tracing::warn!(%error, "could not save settings");
        }
    }

    /// Open the config store and read the current settings.
    ///
    /// A missing or partially-invalid store is never fatal: `get_entry` hands
    /// back defaults for whatever it could not read, and a preview must still
    /// open.
    #[must_use]
    pub fn load() -> Self {
        let Ok(handle) = cosmic_config::Config::new(crate::app::APP_ID, Self::VERSION) else {
            tracing::warn!("cosmic-config unavailable; using default settings");
            return Self::default();
        };

        match Self::get_entry(&handle) {
            Ok(config) => config,
            Err((errors, config)) => {
                for error in errors {
                    tracing::warn!(%error, "using the default for an unreadable setting");
                }
                config
            }
        }
    }

    /// Panel fill opacity, clamped so the panel can never vanish entirely.
    ///
    /// The floor is higher than a launcher's: text rendered over a wallpaper at
    /// 20% opacity is unreadable, and unlike a launcher the user is here to
    /// *read* what is on the panel.
    #[must_use]
    pub fn panel_opacity(&self) -> f32 {
        self.opacity.clamp(0.4, 1.0)
    }

    /// Fraction of the display the panel may occupy, clamped to something that
    /// still leaves the desktop visible behind it.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        self.max_fraction.clamp(0.4, 0.98)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autoplay_modes_differ_only_for_audio() {
        assert!(Autoplay::Always.applies(false));
        assert!(!Autoplay::VideoOnly.applies(false));
        assert!(Autoplay::VideoOnly.applies(true));
        assert!(!Autoplay::Never.applies(true));
    }

    #[test]
    fn opacity_cannot_be_set_low_enough_to_hide_text() {
        let config = Config {
            opacity: 0.0,
            ..Config::default()
        };
        assert!(config.panel_opacity() >= 0.4);
    }

    #[test]
    fn the_panel_never_covers_the_whole_display() {
        let config = Config {
            max_fraction: 5.0,
            ..Config::default()
        };
        assert!(config.fraction() <= 0.98);
    }
}
