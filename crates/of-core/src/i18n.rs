//! The locales the product is translated into.
//!
//! **This module owns the list.** Three other places need the same six values
//! and none of them re-types it by hand except one that cannot avoid it:
//!
//! | Where | How it gets the list |
//! |---|---|
//! | `of_web::i18n` | `pub use of_core::i18n::Locale` — `of-core` cannot depend on `of-web`, so the enum lives here and the HTTP crate re-exports it |
//! | `web/project.inlang/settings.json` | hand-written, and the one copy that cannot be a `use` — it is JSON read by a compiler in another language |
//! | `web/src/lib/locale.ts` | imports `locales` from Paraglide's generated runtime, which is generated *from* that settings file |
//!
//! That leaves exactly two hand-written lists, in two languages, and
//! [`the_console_and_the_server_agree_on_the_locales`] is what stops them
//! drifting. The drift is not theoretical: a locale offered in the console's
//! picker but missing from [`SUPPORTED_LOCALES`] is a `400` the moment someone
//! clicks it, and a locale here but not there is a translation nothing can
//! reach.
//!
//! There is deliberately **no `CHECK` constraint** on `users.locale`. A `CHECK`
//! listing six values makes the seventh locale a migration; validating here
//! makes it a one-line edit, and gives the caller an error that names the valid
//! options rather than a constraint-violation string.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Every locale the console and the browser-facing pages are translated into.
///
/// Ordered `en` first because it is the base and the fallback; the rest are in
/// the order issue #42 named them.
pub const SUPPORTED_LOCALES: [&str; 6] = ["en", "es", "de", "fr", "it", "hi"];

/// One of [`SUPPORTED_LOCALES`], parsed.
///
/// Bare language subtags, not region-qualified: the product is translated into
/// languages, and a region variant (`es-419`, `pt-BR`) would be a new locale
/// file rather than a rework of this type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Locale {
    /// The base locale, and the fallback for anything unresolvable.
    #[default]
    En,
    Es,
    De,
    Fr,
    It,
    Hi,
}

impl Locale {
    /// In the same order as [`SUPPORTED_LOCALES`], which a test pins.
    pub const ALL: [Locale; 6] = [
        Locale::En,
        Locale::Es,
        Locale::De,
        Locale::Fr,
        Locale::It,
        Locale::Hi,
    ];

    /// The BCP 47 subtag — what goes in `<html lang>` and in `users.locale`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Es => "es",
            Locale::De => "de",
            Locale::Fr => "fr",
            Locale::It => "it",
            Locale::Hi => "hi",
        }
    }
}

impl fmt::Display for Locale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Locale {
    type Err = Error;

    /// Case-insensitive, and a region subtag resolves to its language.
    ///
    /// `es-419` and `es-MX` are Spanish. Refusing them would mean a browser
    /// that quite correctly sends a region gets English instead, which is the
    /// opposite of the point — and the same rule is what
    /// `of_web::i18n::negotiate` applies to `Accept-Language`.
    fn from_str(s: &str) -> Result<Self> {
        let primary = s.split(['-', '_']).next().unwrap_or_default();
        let primary = primary.trim().to_ascii_lowercase();

        Locale::ALL
            .into_iter()
            .find(|l| l.as_str() == primary)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "{s:?} is not a supported locale; use one of: {}",
                    SUPPORTED_LOCALES.join(", ")
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_enum_and_the_string_list_are_the_same_list() {
        let from_enum: Vec<&str> = Locale::ALL.iter().map(|l| l.as_str()).collect();
        assert_eq!(from_enum, SUPPORTED_LOCALES.to_vec());
    }

    #[test]
    fn a_region_subtag_resolves_to_its_language() {
        for tag in ["es-419", "es_MX", "ES-mx", "de-CH", "hi-IN"] {
            assert!(tag.parse::<Locale>().is_ok(), "{tag} should parse");
        }
        assert_eq!("es-419".parse::<Locale>().unwrap(), Locale::Es);
        assert_eq!("DE".parse::<Locale>().unwrap(), Locale::De);
    }

    /// The house rule for errors: say what was wrong *and* what would work.
    #[test]
    fn an_unsupported_locale_names_the_supported_ones() {
        let err = "klingon".parse::<Locale>().unwrap_err().to_string();
        for locale in SUPPORTED_LOCALES {
            assert!(err.contains(locale), "{err:?} should name {locale}");
        }
    }

    #[test]
    fn nothing_parses_to_a_locale_by_accident() {
        for junk in ["", "  ", "-", "zz", "english", "e"] {
            assert!(junk.parse::<Locale>().is_err(), "{junk:?} should not parse");
        }
    }

    /// The drift guard between the two hand-written lists.
    ///
    /// `cargo test` runs the binary with its working directory at the *package*
    /// root, not the workspace root, so the obvious `read_to_string("web/...")`
    /// is the implementation that fails. CI runs `cargo test --workspace` from
    /// a full checkout and the Dockerfile only ever runs `cargo build`, so
    /// nothing compiles this test without `web/` on disk.
    #[test]
    fn the_console_and_the_server_agree_on_the_locales() {
        const SETTINGS: &str = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../web/project.inlang/settings.json"
        ));

        let settings: serde_json::Value = serde_json::from_str(SETTINGS).expect("settings.json");

        let console: Vec<&str> = settings["locales"]
            .as_array()
            .expect("locales array")
            .iter()
            .map(|v| v.as_str().expect("locale string"))
            .collect();
        assert_eq!(
            console,
            SUPPORTED_LOCALES.to_vec(),
            "web/project.inlang/settings.json and SUPPORTED_LOCALES disagree. A locale in the \
             console but not the server is a 400 when someone picks it; a locale in the server \
             but not the console is a translation nothing can reach."
        );

        assert_eq!(
            settings["baseLocale"].as_str(),
            Some(Locale::default().as_str()),
            "the console's base locale and Locale::default() have to be the same fallback"
        );
    }
}
