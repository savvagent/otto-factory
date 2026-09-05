//! Language for the two pages a browser renders without the console.
//!
//! `/oauth/authorize`'s consent screen and its error page are HTML this crate
//! writes; there is no client-side JS to swap strings, so the language has to
//! be decided server-side, per request.
//!
//! **The enum is not defined here.** `df_core::i18n::Locale` owns the list —
//! `df-core` cannot depend on `df-web`, so this direction is the only one
//! available, and it is also the right one: which languages the product speaks
//! is a domain fact, not an HTTP one. This module owns the two things that
//! genuinely are HTTP concerns: parsing `Accept-Language`, and the strings for
//! these two pages.
//!
//! The console's own strings do **not** live here. They are compiled by
//! Paraglide from `web/messages/*.json`, because the console is a static bundle
//! that never asks the server for a word. Twenty-odd strings across two pages
//! did not justify a second compiler toolchain, and the two surfaces share no
//! keys anyway.

pub use df_core::i18n::Locale;

/// Pick a locale from an `Accept-Language` header.
///
/// RFC 9110 §12.5.4. Weighted, so `de;q=0.9, en;q=1.0` is English even though
/// German comes first.
///
/// Three details are easy to get wrong and all three are tested:
///
/// - **`q=0` means "not acceptable"**, not "rank last". An entry with zero
///   weight is dropped rather than sorted to the bottom, or `en;q=0, de` would
///   still be able to return English.
/// - **A region subtag selects its language.** `es-419` and `es-MX` are
///   Spanish. Refusing them would hand English to a browser that was perfectly
///   clear about what it wanted.
/// - **Malformed entries are skipped, never fatal.** This header is attacker-
///   controlled on an unauthenticated path; a parse error must not be able to
///   turn a consent screen into a `500`.
///
/// Anything unresolvable — absent, empty, `*`, entirely junk — is [`Locale::En`],
/// which is the base locale and the language every other string falls back to.
pub fn negotiate(accept_language: Option<&str>) -> Locale {
    let Some(header) = accept_language else {
        return Locale::En;
    };

    let mut candidates: Vec<(f32, usize, Locale)> = Vec::new();

    for (position, entry) in header.split(',').enumerate() {
        let mut parts = entry.split(';');
        let Some(tag) = parts.next().map(str::trim) else {
            continue;
        };
        if tag.is_empty() || tag == "*" {
            continue;
        }

        // Default weight is 1.0. A `q=` that does not parse is treated as
        // absent rather than as zero: the sender clearly wanted this language,
        // and reading a typo as a refusal is the more surprising failure.
        let weight = parts
            .find_map(|p| {
                let p = p.trim();
                p.strip_prefix("q=").or_else(|| p.strip_prefix("Q="))
            })
            .and_then(|q| q.trim().parse::<f32>().ok())
            .unwrap_or(1.0);

        // q=0 is "not acceptable" — drop it rather than ranking it last.
        if weight <= 0.0 {
            continue;
        }

        if let Ok(locale) = tag.parse::<Locale>() {
            candidates.push((weight, position, locale));
        }
    }

    // Descending weight; ties broken by the order the client listed them, which
    // is the client's own statement of preference.
    candidates.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });

    candidates
        .first()
        .map(|(_, _, locale)| *locale)
        .unwrap_or(Locale::En)
}

/// Every string the two browser-facing pages render.
///
/// An enum rather than string keys so [`msg`] is an exhaustive `match` on both
/// axes: a new key with no Hindi translation **fails to compile**, which is the
/// only way a table like this stays complete without a test watching it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    // The consent screen.
    ConsentTitle,
    ConsentHeading,
    /// Takes the redirect host.
    ConsentAsking,
    /// Takes the client's self-asserted name.
    ConsentCallsItself,
    ConsentWarnName,
    ConsentItIsAskingTo,
    ConsentOrganization,
    ConsentOrgScopeNote,
    ConsentAllow,
    ConsentCancel,
    /// Takes the signed-in address.
    ConsentSignedInAs,

    // The error page.
    ErrorTitle,
    ErrorNothingAuthorized,
    ErrorNoOrgTitle,
    /// Takes the client's name.
    ErrorNoOrgBody,

    // Scope descriptions, in the order `oauth::KNOWN_SCOPES` lists them.
    ScopeJobsRead,
    ScopeJobsWrite,
    ScopeReposRead,
    ScopeReposWrite,
    ScopeMessages,
    ScopeTrackers,
    ScopeOrgAdmin,
}

/// The message table.
///
/// Placeholders are `{}` and are filled by the caller, which keeps this a
/// `&'static str` table with no allocation and no format machinery.
///
/// Product nouns are not translated: `dark-factory`, JIRA and GitHub are names,
/// and "passkey" is the term the rest of the product uses in every language.
pub fn msg(locale: Locale, key: Key) -> &'static str {
    use Key::*;
    use Locale::*;

    match (key, locale) {
        // ------------------------------------------------------- consent
        (ConsentTitle, En) => "Authorize access",
        (ConsentTitle, Es) => "Autorizar acceso",
        (ConsentTitle, De) => "Zugriff autorisieren",
        (ConsentTitle, Fr) => "Autoriser l'accès",
        (ConsentTitle, It) => "Autorizza l'accesso",
        (ConsentTitle, Hi) => "पहुँच अधिकृत करें",

        (ConsentHeading, En) => "Authorize access to dark-factory",
        (ConsentHeading, Es) => "Autorizar acceso a dark-factory",
        (ConsentHeading, De) => "Zugriff auf dark-factory autorisieren",
        (ConsentHeading, Fr) => "Autoriser l'accès à dark-factory",
        (ConsentHeading, It) => "Autorizza l'accesso a dark-factory",
        (ConsentHeading, Hi) => "dark-factory तक पहुँच अधिकृत करें",

        (ConsentAsking, En) => "An application running on {} is asking to connect to your queue.",
        (ConsentAsking, Es) => "Una aplicación que se ejecuta en {} solicita conectarse a tu cola.",
        (ConsentAsking, De) => {
            "Eine Anwendung auf {} möchte sich mit deiner Warteschlange verbinden."
        }
        (ConsentAsking, Fr) => {
            "Une application exécutée sur {} demande à se connecter à votre file d'attente."
        }
        (ConsentAsking, It) => {
            "Un'applicazione in esecuzione su {} chiede di connettersi alla tua coda."
        }
        (ConsentAsking, Hi) => "{} पर चल रहा एक एप्लिकेशन आपकी कतार से जुड़ने की अनुमति माँग रहा है।",

        (ConsentCallsItself, En) => "It calls itself {}.",
        (ConsentCallsItself, Es) => "Se identifica como {}.",
        (ConsentCallsItself, De) => "Sie nennt sich {}.",
        (ConsentCallsItself, Fr) => "Elle se présente comme {}.",
        (ConsentCallsItself, It) => "Si presenta come {}.",
        (ConsentCallsItself, Hi) => "यह स्वयं को {} कहता है।",

        (ConsentWarnName, En) => {
            "Only continue if you started this from that application. \
             Any application can choose its own name."
        }
        (ConsentWarnName, Es) => {
            "Continúa solo si iniciaste esto desde esa aplicación. \
             Cualquier aplicación puede elegir su propio nombre."
        }
        (ConsentWarnName, De) => {
            "Fahre nur fort, wenn du dies aus dieser Anwendung heraus gestartet hast. \
             Jede Anwendung kann ihren Namen selbst wählen."
        }
        (ConsentWarnName, Fr) => {
            "Ne continuez que si vous avez lancé cette demande depuis cette application. \
             N'importe quelle application peut choisir son propre nom."
        }
        (ConsentWarnName, It) => {
            "Continua solo se hai avviato questa richiesta da quell'applicazione. \
             Qualsiasi applicazione può scegliere il proprio nome."
        }
        (ConsentWarnName, Hi) => {
            "आगे तभी बढ़ें जब आपने इसे उसी एप्लिकेशन से शुरू किया हो। \
             कोई भी एप्लिकेशन अपना नाम स्वयं चुन सकता है।"
        }

        (ConsentItIsAskingTo, En) => "It is asking to:",
        (ConsentItIsAskingTo, Es) => "Solicita permiso para:",
        (ConsentItIsAskingTo, De) => "Sie bittet um die Berechtigung:",
        (ConsentItIsAskingTo, Fr) => "Elle demande à :",
        (ConsentItIsAskingTo, It) => "Chiede di:",
        (ConsentItIsAskingTo, Hi) => "यह निम्नलिखित की अनुमति माँग रहा है:",

        (ConsentOrganization, En) => "Organization",
        (ConsentOrganization, Es) => "Organización",
        (ConsentOrganization, De) => "Organisation",
        (ConsentOrganization, Fr) => "Organisation",
        (ConsentOrganization, It) => "Organizzazione",
        (ConsentOrganization, Hi) => "संगठन",

        (ConsentOrgScopeNote, En) => {
            "The token will act in this organization only, and cannot be moved to another."
        }
        (ConsentOrgScopeNote, Es) => {
            "El token actuará solo en esta organización y no puede trasladarse a otra."
        }
        (ConsentOrgScopeNote, De) => {
            "Das Token gilt nur in dieser Organisation und kann nicht auf eine andere \
             übertragen werden."
        }
        (ConsentOrgScopeNote, Fr) => {
            "Le jeton n'agira que dans cette organisation et ne peut pas être transféré \
             à une autre."
        }
        (ConsentOrgScopeNote, It) => {
            "Il token agirà solo in questa organizzazione e non può essere spostato in un'altra."
        }
        (ConsentOrgScopeNote, Hi) => {
            "यह टोकन केवल इसी संगठन में काम करेगा और इसे किसी अन्य संगठन में नहीं ले जाया जा सकता।"
        }

        (ConsentAllow, En) => "Allow access",
        (ConsentAllow, Es) => "Permitir acceso",
        (ConsentAllow, De) => "Zugriff erlauben",
        (ConsentAllow, Fr) => "Autoriser l'accès",
        (ConsentAllow, It) => "Consenti l'accesso",
        (ConsentAllow, Hi) => "पहुँच की अनुमति दें",

        (ConsentCancel, En) => "Cancel",
        (ConsentCancel, Es) => "Cancelar",
        (ConsentCancel, De) => "Abbrechen",
        (ConsentCancel, Fr) => "Annuler",
        (ConsentCancel, It) => "Annulla",
        (ConsentCancel, Hi) => "रद्द करें",

        (ConsentSignedInAs, En) => "Signed in as {}.",
        (ConsentSignedInAs, Es) => "Sesión iniciada como {}.",
        (ConsentSignedInAs, De) => "Angemeldet als {}.",
        (ConsentSignedInAs, Fr) => "Connecté en tant que {}.",
        (ConsentSignedInAs, It) => "Accesso effettuato come {}.",
        (ConsentSignedInAs, Hi) => "{} के रूप में साइन इन किया गया।",

        // --------------------------------------------------------- errors
        (ErrorTitle, En) => "This request could not be authorized",
        (ErrorTitle, Es) => "No se pudo autorizar esta solicitud",
        (ErrorTitle, De) => "Diese Anfrage konnte nicht autorisiert werden",
        (ErrorTitle, Fr) => "Cette demande n'a pas pu être autorisée",
        (ErrorTitle, It) => "Non è stato possibile autorizzare questa richiesta",
        (ErrorTitle, Hi) => "इस अनुरोध को अधिकृत नहीं किया जा सका",

        (ErrorNothingAuthorized, En) => "Nothing has been authorized. You can close this window.",
        (ErrorNothingAuthorized, Es) => "No se ha autorizado nada. Puedes cerrar esta ventana.",
        (ErrorNothingAuthorized, De) => {
            "Es wurde nichts autorisiert. Du kannst dieses Fenster schließen."
        }
        (ErrorNothingAuthorized, Fr) => "Rien n'a été autorisé. Vous pouvez fermer cette fenêtre.",
        (ErrorNothingAuthorized, It) => {
            "Non è stato autorizzato nulla. Puoi chiudere questa finestra."
        }
        (ErrorNothingAuthorized, Hi) => "कुछ भी अधिकृत नहीं किया गया। आप यह विंडो बंद कर सकते हैं।",

        (ErrorNoOrgTitle, En) => "No organization yet",
        (ErrorNoOrgTitle, Es) => "Todavía no hay ninguna organización",
        (ErrorNoOrgTitle, De) => "Noch keine Organisation",
        (ErrorNoOrgTitle, Fr) => "Aucune organisation pour le moment",
        (ErrorNoOrgTitle, It) => "Nessuna organizzazione ancora",
        (ErrorNoOrgTitle, Hi) => "अभी तक कोई संगठन नहीं",

        (ErrorNoOrgBody, En) => {
            "{} is asking for access, but your account is not in any organization yet. \
             Create one in the console first — a token is always scoped to exactly one."
        }
        (ErrorNoOrgBody, Es) => {
            "{} solicita acceso, pero tu cuenta aún no pertenece a ninguna organización. \
             Crea una en la consola primero: un token siempre pertenece exactamente a una."
        }
        (ErrorNoOrgBody, De) => {
            "{} fragt Zugriff an, aber dein Konto gehört noch zu keiner Organisation. \
             Lege zuerst eine in der Konsole an — ein Token gilt immer für genau eine."
        }
        (ErrorNoOrgBody, Fr) => {
            "{} demande l'accès, mais votre compte n'appartient encore à aucune organisation. \
             Créez-en une dans la console : un jeton est toujours limité à une seule."
        }
        (ErrorNoOrgBody, It) => {
            "{} chiede l'accesso, ma il tuo account non appartiene ancora a nessuna \
             organizzazione. Creane una nella console: un token è sempre limitato a una sola."
        }
        (ErrorNoOrgBody, Hi) => {
            "{} पहुँच माँग रहा है, लेकिन आपका खाता अभी किसी संगठन में नहीं है। \
             पहले कंसोल में एक संगठन बनाएँ — एक टोकन हमेशा ठीक एक संगठन तक सीमित होता है।"
        }

        // --------------------------------------------------------- scopes
        (ScopeJobsRead, En) => "See the work queue and job details",
        (ScopeJobsRead, Es) => "Ver la cola de trabajo y los detalles de los trabajos",
        (ScopeJobsRead, De) => "Die Warteschlange und Auftragsdetails einsehen",
        (ScopeJobsRead, Fr) => "Voir la file d'attente et le détail des tâches",
        (ScopeJobsRead, It) => "Vedere la coda di lavoro e i dettagli dei processi",
        (ScopeJobsRead, Hi) => "कार्य कतार और कार्य विवरण देखना",

        (ScopeJobsWrite, En) => "Create, claim, update, and complete jobs",
        (ScopeJobsWrite, Es) => "Crear, reclamar, actualizar y completar trabajos",
        (ScopeJobsWrite, De) => "Aufträge anlegen, übernehmen, aktualisieren und abschließen",
        (ScopeJobsWrite, Fr) => "Créer, réclamer, mettre à jour et terminer des tâches",
        (ScopeJobsWrite, It) => "Creare, prendere in carico, aggiornare e completare processi",
        (ScopeJobsWrite, Hi) => "कार्य बनाना, लेना, अद्यतन करना और पूरा करना",

        (ScopeReposRead, En) => "See which repositories are registered",
        (ScopeReposRead, Es) => "Ver qué repositorios están registrados",
        (ScopeReposRead, De) => "Sehen, welche Repositories registriert sind",
        (ScopeReposRead, Fr) => "Voir les dépôts enregistrés",
        (ScopeReposRead, It) => "Vedere quali repository sono registrati",
        (ScopeReposRead, Hi) => "देखना कि कौन-से रिपॉज़िटरी पंजीकृत हैं",

        (ScopeReposWrite, En) => "Register repositories and change their settings",
        (ScopeReposWrite, Es) => "Registrar repositorios y cambiar su configuración",
        (ScopeReposWrite, De) => "Repositories registrieren und ihre Einstellungen ändern",
        (ScopeReposWrite, Fr) => "Enregistrer des dépôts et modifier leurs paramètres",
        (ScopeReposWrite, It) => "Registrare repository e modificarne le impostazioni",
        (ScopeReposWrite, Hi) => "रिपॉज़िटरी पंजीकृत करना और उनकी सेटिंग्स बदलना",

        (ScopeMessages, En) => "Read and send messages between agents",
        (ScopeMessages, Es) => "Leer y enviar mensajes entre agentes",
        (ScopeMessages, De) => "Nachrichten zwischen Agenten lesen und senden",
        (ScopeMessages, Fr) => "Lire et envoyer des messages entre agents",
        (ScopeMessages, It) => "Leggere e inviare messaggi tra agenti",
        (ScopeMessages, Hi) => "एजेंटों के बीच संदेश पढ़ना और भेजना",

        (ScopeTrackers, En) => "Link jobs to issues in JIRA or GitHub",
        (ScopeTrackers, Es) => "Vincular trabajos con incidencias de JIRA o GitHub",
        (ScopeTrackers, De) => "Aufträge mit Vorgängen in JIRA oder GitHub verknüpfen",
        (ScopeTrackers, Fr) => "Associer des tâches à des tickets JIRA ou GitHub",
        (ScopeTrackers, It) => "Collegare processi a issue di JIRA o GitHub",
        (ScopeTrackers, Hi) => "कार्यों को JIRA या GitHub के मुद्दों से जोड़ना",

        (ScopeOrgAdmin, En) => "Administer the organization: members, teams, and connections",
        (ScopeOrgAdmin, Es) => "Administrar la organización: miembros, equipos y conexiones",
        (ScopeOrgAdmin, De) => "Die Organisation verwalten: Mitglieder, Teams und Verbindungen",
        (ScopeOrgAdmin, Fr) => "Administrer l'organisation : membres, équipes et connexions",
        (ScopeOrgAdmin, It) => "Amministrare l'organizzazione: membri, team e connessioni",
        (ScopeOrgAdmin, Hi) => "संगठन का प्रशासन: सदस्य, टीमें और कनेक्शन",
    }
}

/// Fill a single `{}` placeholder.
///
/// Deliberately not `format!`: the table is `&'static str`, the caller has
/// already escaped the value, and a real format machinery here would invite
/// somebody to pass an unescaped one.
pub fn fill(template: &str, value: &str) -> String {
    template.replacen("{}", value, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_header_and_junk_headers_are_english() {
        for header in [None, Some(""), Some("*"), Some(";;;"), Some("q=0.5")] {
            assert_eq!(negotiate(header), Locale::En, "header {header:?}");
        }
    }

    #[test]
    fn a_single_supported_language_wins() {
        assert_eq!(negotiate(Some("de")), Locale::De);
        assert_eq!(negotiate(Some("hi")), Locale::Hi);
    }

    #[test]
    fn weights_decide_and_are_not_merely_document_order() {
        // German is listed first but explicitly less wanted than English.
        assert_eq!(negotiate(Some("de;q=0.7, en;q=0.9")), Locale::En);
        assert_eq!(negotiate(Some("en;q=0.4, fr;q=0.8")), Locale::Fr);
    }

    #[test]
    fn equal_weights_fall_back_to_the_order_the_client_listed() {
        assert_eq!(negotiate(Some("it, fr")), Locale::It);
        assert_eq!(negotiate(Some("fr;q=0.8, it;q=0.8")), Locale::Fr);
    }

    /// `q=0` means "not acceptable", not "least preferred".
    #[test]
    fn a_zero_weight_removes_a_language_rather_than_ranking_it_last() {
        assert_eq!(negotiate(Some("en;q=0, de")), Locale::De);
        // Every supported language refused, so nothing is left to choose.
        assert_eq!(negotiate(Some("de;q=0, es;q=0")), Locale::En);
    }

    #[test]
    fn a_region_subtag_selects_its_language() {
        assert_eq!(negotiate(Some("es-419")), Locale::Es);
        assert_eq!(negotiate(Some("de-CH,de;q=0.9")), Locale::De);
        assert_eq!(negotiate(Some("hi-IN")), Locale::Hi);
    }

    #[test]
    fn unsupported_languages_are_skipped_for_a_supported_one_further_down() {
        assert_eq!(negotiate(Some("ja, ko;q=0.9, it;q=0.5")), Locale::It);
        assert_eq!(negotiate(Some("ja, ko")), Locale::En);
    }

    /// This header is attacker-controlled on a path that has not authenticated
    /// yet. A parse failure must degrade, never panic.
    #[test]
    fn malformed_entries_are_skipped_rather_than_fatal() {
        assert_eq!(negotiate(Some("de;q=banana")), Locale::De, "bad q is not 0");
        assert_eq!(negotiate(Some(",,,de,,,")), Locale::De);
        assert_eq!(negotiate(Some("de;;;q=0.9;;;")), Locale::De);
        assert_eq!(negotiate(Some(&"a".repeat(10_000))), Locale::En);
        assert_eq!(negotiate(Some("de;q=-1, es")), Locale::Es, "negative is 0");
    }

    /// The table is exhaustive by construction — this proves it is also
    /// *populated*, i.e. nobody translated a key to the empty string.
    #[test]
    fn every_key_has_a_non_empty_string_in_every_locale() {
        use Key::*;
        let keys = [
            ConsentTitle,
            ConsentHeading,
            ConsentAsking,
            ConsentCallsItself,
            ConsentWarnName,
            ConsentItIsAskingTo,
            ConsentOrganization,
            ConsentOrgScopeNote,
            ConsentAllow,
            ConsentCancel,
            ConsentSignedInAs,
            ErrorTitle,
            ErrorNothingAuthorized,
            ErrorNoOrgTitle,
            ErrorNoOrgBody,
            ScopeJobsRead,
            ScopeJobsWrite,
            ScopeReposRead,
            ScopeReposWrite,
            ScopeMessages,
            ScopeTrackers,
            ScopeOrgAdmin,
        ];

        for locale in Locale::ALL {
            for key in keys {
                assert!(
                    !msg(locale, key).trim().is_empty(),
                    "{key:?} is empty in {locale}"
                );
            }
        }
    }

    /// A message with a placeholder must keep it in every language, or the
    /// value it was meant to carry is silently dropped.
    #[test]
    fn placeholders_survive_translation() {
        use Key::*;
        for key in [
            ConsentAsking,
            ConsentCallsItself,
            ConsentSignedInAs,
            ErrorNoOrgBody,
        ] {
            for locale in Locale::ALL {
                assert!(
                    msg(locale, key).contains("{}"),
                    "{key:?} lost its placeholder in {locale}"
                );
            }
        }
    }

    #[test]
    fn fill_replaces_exactly_one_placeholder() {
        assert_eq!(fill("Signed in as {}.", "rob"), "Signed in as rob.");
        assert_eq!(fill("no placeholder", "rob"), "no placeholder");
    }
}
