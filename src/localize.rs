// Copyright 2026 entro314-labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Translated strings.
//!
//! The same arrangement every COSMIC application uses: Fluent catalogues under
//! `i18n/`, embedded into the binary by `rust-embed` so there is nothing to
//! install alongside it, and reached through an [`fl!`] macro that resolves the
//! message id at compile time — a typo in a key is a build error rather than a
//! string that renders as its own name.
//!
//! [`localize`] has to run before the first string is asked for. It is called
//! from `main`, once, before the application starts.

use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::{DefaultLocalizer, LanguageLoader, Localizer};
use rust_embed::RustEmbed;
use std::sync::LazyLock;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();

    loader
        .load_fallback_language(&Localizations)
        .expect("the fallback language is compiled in and must load");

    loader
});

/// Look up a translated string by its Fluent id.
#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $message_id)
    }};

    ($message_id:literal, $($args:expr),*) => {{
        i18n_embed_fl::fl!($crate::localize::LANGUAGE_LOADER, $message_id, $($args), *)
    }};
}

/// Select the catalogue matching the desktop's configured languages.
pub fn localize() {
    let localizer = Box::from(DefaultLocalizer::new(&*LANGUAGE_LOADER, &Localizations));
    let requested = i18n_embed::DesktopLanguageRequester::requested_languages();

    if let Err(error) = Localizer::select(&*localizer, &requested) {
        // Not fatal, and deliberately not a hard error: the fallback catalogue
        // is already loaded, so the previewer opens in English rather than not
        // opening at all.
        tracing::warn!(%error, "falling back to the default language");
    }
}
