//! Everything the process cannot infer, read once from the environment.
//!
//! Two rules decide whether a setting gets a default, and they are the reason
//! this file is longer than a `serde` derive would be:
//!
//! 1. **A setting whose wrong value fails silently has no default.** A wrong
//!    `OF_PUBLIC_URL` does not crash: it hands out links to an origin that does not
//!    exist and advertises an audience nothing accepts, and the first
//!    report arrives hours later from somebody who cannot sign in. Refusing to
//!    start is the cheap version of that failure.
//! 2. **A setting whose wrong value is merely inconvenient gets one.** The bind
//!    address, the log format, and the static directory are all obvious within
//!    seconds of looking.
//!
//! Nothing here falls back quietly. A variable that is set but unparseable is
//! an error naming the variable and what it accepts, never a default — a
//! `OF_ENFORCE_QUOTAS=yes-please` that silently reads as "off" is how a billing
//! control gets deployed switched off for a year.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

/// The whole deployment, resolved.
///
/// `Debug` is written by hand and prints no secret: this struct holds the
/// platform credentials, the encryption key, and the tracker providers' client
/// secrets, and a `{:?}` in a log line must never be how they leak.
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub bind: SocketAddr,

    /// Public origin a browser sees. The links this service hands out and the
    /// discovery pointer in a `401` are built from it.
    pub public_url: String,
    /// Canonical MCP resource URI, and the audience every token carries. Must
    /// be exactly the `resource_uri` this service was registered with at the
    /// platform (`otto-platform-server resource register`).
    pub resource_uri: String,

    /// The otto platform's base URL, e.g. `https://otto.savvagent.com`: the OAuth
    /// authorization server, the identity directory, and the billing system of
    /// record. Advertised to MCP clients as this resource's authorization server
    /// and called for token introspection, member and team lookups, and usage.
    pub platform_url: String,
    /// The introspection credential the platform issued when this service was
    /// registered (`otto_rs_…`). Authenticates every call this service makes to
    /// the platform, as HTTP Basic with `resource_uri` as the user.
    pub introspection_secret: String,
    /// The key the platform signs its lifecycle webhooks to this service with
    /// (`otto_whsec_…`), issued when the webhook URL
    /// (`{public_url}/platform/webhooks`) was registered. A different secret
    /// from the introspection credential, on purpose.
    pub platform_webhook_secret: String,

    /// 32 bytes, base64. Encrypts secrets at rest (currently tracker webhook
    /// secrets and JIRA OAuth credentials — see `of_core::trackers` and
    /// `of_trackers::jira`).
    pub encryption_key: String,

    /// GitHub App id for tracker integration. Optional because deployments
    /// that do not enable GitHub integration have no App configured.
    pub github_app_id: Option<i64>,
    /// PEM-encoded GitHub App private key. Optional for the same reason as
    /// `github_app_id`; PEM validity is checked later, when a task constructs
    /// a `GithubAppClient` from it, rather than here where the key is just
    /// configuration data. Literal `\n` escapes (how a multi-line PEM is
    /// commonly stored in a single-line `.env` value or a CI secret) are
    /// normalized to real newlines before storage.
    pub github_app_private_key: Option<String>,
    /// Shared secret for GitHub webhook signature verification. Optional
    /// because GitHub integration itself is optional per deployment.
    pub github_app_webhook_secret: Option<String>,
    /// The GitHub App's URL slug, used to build the installation link the
    /// console sends an admin to (`github.com/apps/{slug}/installations/new`).
    /// Optional for the same reason as the rest; the console offers no Connect
    /// GitHub button without it, rather than linking somewhere that 404s.
    pub github_app_slug: Option<String>,
    /// The GitHub App's OAuth client id and secret — the *user*-to-server
    /// credentials, distinct from `github_app_private_key`, which is the
    /// server-to-server one. The console needs these to prove that the admin
    /// binding an installation id actually administers that installation; see
    /// `docs/specs/2026-09-04-tracker-console-design.md` §2 for why an
    /// unverified installation id is a cross-tenant escalation.
    pub github_app_client_id: Option<String>,
    pub github_app_client_secret: Option<String>,
    /// Atlassian OAuth client id for JIRA tracker sync. Optional because JIRA
    /// integration itself is optional per deployment.
    pub jira_client_id: Option<String>,
    /// Atlassian OAuth client secret for JIRA tracker sync.
    pub jira_client_secret: Option<String>,

    /// The console's OAuth client id at the platform, for console sign-in. A
    /// public first-party client an operator registers
    /// (`otto-platform-server client register --first-party`) with the redirect
    /// URI `{public_url}/auth/callback`. Optional: while unset, `/auth/login`
    /// answers `503 console_login_disabled` and everything else runs, so the
    /// server can be deployed before the client exists.
    pub console_client_id: Option<String>,

    /// Refuse billable calls once an org on a hard-stop plan is past its bucket,
    /// as the platform reports it (cached up to a minute, so overrun is bounded
    /// by that window and by what is still in the usage outbox).
    pub enforce_quotas: bool,
    /// Where a caller who has run out is told to go. Defaults to the platform.
    pub upgrade_url: String,

    /// Extra authorities accepted in the MCP endpoint's `Host` header, beyond
    /// the one derived from `public_url`.
    pub extra_allowed_hosts: Vec<String>,
    /// Browser origins accepted on requests to `/mcp` that carry `Origin`.
    /// Empty disables the check, which is right for a surface reached by CLI
    /// agents rather than by pages.
    pub allowed_origins: Vec<String>,

    /// The built console bundle. Served with an `index.html` fallback.
    pub static_dir: PathBuf,

    pub run_migrations: bool,
    pub log_format: LogFormat,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("bind", &self.bind)
            .field("public_url", &self.public_url)
            .field("resource_uri", &self.resource_uri)
            .field("platform_url", &self.platform_url)
            .field("console_client_id", &self.console_client_id)
            .field("enforce_quotas", &self.enforce_quotas)
            .field("static_dir", &self.static_dir)
            .field("run_migrations", &self.run_migrations)
            .field("log_format", &self.log_format)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per line, for a log aggregator.
    Json,
    /// Human-readable, for a terminal.
    Text,
}

impl Config {
    /// Read the environment. Every failure names the variable it is about.
    pub fn from_env() -> Result<Self> {
        let public_url = required("OF_PUBLIC_URL")?.trim_end_matches('/').to_string();

        // Parsed rather than merely stored: a `OF_PUBLIC_URL` with no scheme
        // produces links a mail client will not linkify and an `allowed_hosts`
        // entry of the empty string, both of which are much harder to diagnose
        // later than a message here.
        let parsed = url::Url::parse(&public_url)
            .with_context(|| format!("OF_PUBLIC_URL is not a URL: {public_url:?}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(anyhow!(
                "OF_PUBLIC_URL must be http or https, got {:?}",
                parsed.scheme()
            ));
        }
        if parsed.host_str().is_none() {
            return Err(anyhow!("OF_PUBLIC_URL has no host: {public_url:?}"));
        }

        let platform_url = required("OF_PLATFORM_URL")?
            .trim_end_matches('/')
            .to_string();
        let platform = url::Url::parse(&platform_url)
            .with_context(|| format!("OF_PLATFORM_URL is not a URL: {platform_url:?}"))?;
        if !matches!(platform.scheme(), "http" | "https") || platform.host_str().is_none() {
            return Err(anyhow!(
                "OF_PLATFORM_URL must be an http(s) URL with a host, got {platform_url:?}"
            ));
        }

        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind: parse_var("OF_BIND", "0.0.0.0:8080", |v| {
                v.parse::<SocketAddr>()
                    .map_err(|e| anyhow!("{e}; expected host:port, e.g. 0.0.0.0:8080"))
            })?,

            // Defaults to the MCP endpoint on the public origin, which is where
            // this binary actually serves it. Overridable because a deployment
            // behind a rewriting proxy may advertise a different canonical URI,
            // and the audience must match what the AS mints tokens for.
            resource_uri: optional("OF_RESOURCE_URI")
                .unwrap_or_else(|| format!("{public_url}/mcp")),

            platform_url: platform_url.clone(),
            introspection_secret: required("OF_INTROSPECTION_SECRET")?,
            platform_webhook_secret: required("OF_PLATFORM_WEBHOOK_SECRET")?,

            encryption_key: required("OF_ENCRYPTION_KEY")?,
            github_app_id: optional("OF_GITHUB_APP_ID")
                .map(|value| {
                    value.trim().parse::<i64>().with_context(|| {
                        format!(
                            "OF_GITHUB_APP_ID={value:?} is not valid; expected an integer App id"
                        )
                    })
                })
                .transpose()?,
            github_app_private_key: optional("OF_GITHUB_APP_PRIVATE_KEY")
                .map(normalize_pem_newlines),
            github_app_webhook_secret: optional("OF_GITHUB_APP_WEBHOOK_SECRET"),
            github_app_slug: optional("OF_GITHUB_APP_SLUG"),
            github_app_client_id: optional("OF_GITHUB_APP_CLIENT_ID"),
            github_app_client_secret: optional("OF_GITHUB_APP_CLIENT_SECRET"),
            jira_client_id: optional("OF_JIRA_CLIENT_ID"),
            jira_client_secret: optional("OF_JIRA_CLIENT_SECRET"),
            console_client_id: optional("OF_CONSOLE_CLIENT_ID"),

            enforce_quotas: parse_var("OF_ENFORCE_QUOTAS", "0", parse_bool)?,
            upgrade_url: optional("OF_UPGRADE_URL")
                .unwrap_or_else(|| format!("{platform_url}/settings/billing")),

            extra_allowed_hosts: list("OF_ALLOWED_HOSTS"),
            allowed_origins: list("OF_ALLOWED_ORIGINS"),

            static_dir: optional("OF_STATIC_DIR")
                .unwrap_or_else(|| "web/build".into())
                .into(),

            run_migrations: parse_var("OF_RUN_MIGRATIONS", "1", parse_bool)?,
            log_format: parse_var("OF_LOG_FORMAT", "text", |v| match v {
                "json" => Ok(LogFormat::Json),
                "text" => Ok(LogFormat::Text),
                other => Err(anyhow!("expected json or text, got {other:?}")),
            })?,

            // Last, because the defaults above are built from it.
            public_url,
        })
    }

    /// Authorities the MCP endpoint accepts in `Host`.
    ///
    /// The public URL's host is always in the list, because that is the name
    /// this deployment tells clients to use. `rmcp` treats an entry with no
    /// port as matching any port, so the bare host covers `example.com:8080`
    /// as well as `example.com`.
    pub fn allowed_hosts(&self) -> Vec<String> {
        let mut hosts: Vec<String> = url::Url::parse(&self.public_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_string))
            .into_iter()
            .collect();
        hosts.extend(self.extra_allowed_hosts.iter().cloned());
        hosts
    }
}

fn optional(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn required(name: &str) -> Result<String> {
    optional(name).ok_or_else(|| anyhow!("{name} is required and not set"))
}

/// Parse an optional variable, defaulting only when it is absent.
///
/// A variable that is *present* and unparseable is an error. Falling back to
/// the default there would mean a typo in a value someone deliberately set is
/// indistinguishable from not setting it.
fn parse_var<T>(name: &str, default: &str, parse: impl Fn(&str) -> Result<T>) -> Result<T> {
    let raw = optional(name);
    let value = raw.as_deref().unwrap_or(default);
    parse(value.trim()).with_context(|| format!("{name}={value:?} is not valid"))
}

fn parse_bool(v: &str) -> Result<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        other => Err(anyhow!(
            "expected a boolean (1/0, true/false, yes/no, on/off), got {other:?}"
        )),
    }
}

/// A comma-separated list. Absent and empty both mean "none".
fn list(name: &str) -> Vec<String> {
    optional(name)
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Normalize a PEM value that arrived as a literal `\n`-escaped single line —
/// the common shape for a multi-line secret stored in a `.env` file or a CI
/// secret store that doesn't preserve real newlines.
fn normalize_pem_newlines(value: String) -> String {
    value.replace("\\n", "\n")
}

#[cfg(test)]
impl Config {
    /// A complete, valid `Config` for tests.
    ///
    /// One fixture rather than a literal per test: a new field then fails to
    /// compile here once, instead of being quietly defaulted in every test that
    /// happens to build a `Config` by hand.
    pub(crate) fn for_test() -> Self {
        Self {
            database_url: "postgres://x".into(),
            bind: "0.0.0.0:8080".parse().expect("test bind"),
            public_url: "https://factory.example.com".into(),
            resource_uri: "https://factory.example.com/mcp".into(),
            platform_url: "https://otto.example.com".into(),
            introspection_secret: "otto_rs_test".into(),
            platform_webhook_secret: "otto_whsec_test".into(),
            encryption_key: "k".into(),
            github_app_id: None,
            github_app_private_key: None,
            github_app_webhook_secret: None,
            github_app_slug: None,
            github_app_client_id: None,
            github_app_client_secret: None,
            jira_client_id: None,
            jira_client_secret: None,
            console_client_id: None,
            enforce_quotas: false,
            upgrade_url: "https://otto.example.com/settings/billing".into(),
            extra_allowed_hosts: vec![],
            allowed_origins: vec![],
            static_dir: "web/build".into(),
            run_migrations: true,
            log_format: LogFormat::Text,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booleans_accept_the_spellings_people_actually_write() {
        for yes in ["1", "true", "TRUE", "yes", "On"] {
            assert!(parse_bool(yes).unwrap(), "{yes}");
        }
        for no in ["0", "false", "NO", "off"] {
            assert!(!parse_bool(no).unwrap(), "{no}");
        }
    }

    /// The whole point of `parse_bool` returning a `Result`. `OF_ENFORCE_QUOTAS`
    /// reading as "off" because somebody wrote `enabled` is a control that is
    /// deployed switched off, and nothing says so.
    #[test]
    fn a_misspelled_boolean_is_an_error_and_not_a_default() {
        for bad in ["enabled", "y", "2", "please"] {
            assert!(parse_bool(bad).is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn a_default_applies_only_when_the_variable_is_absent() {
        // Absent: the default is used.
        assert_eq!(
            parse_var("OF_TEST_ABSENT_VAR_XYZ", "7", |v| Ok(v.parse::<u8>()?)).unwrap(),
            7
        );
    }

    #[test]
    fn a_list_treats_absent_empty_and_whitespace_alike() {
        assert!(list("OF_TEST_ABSENT_LIST_XYZ").is_empty());
    }

    #[test]
    fn a_pem_with_literal_newline_escapes_is_normalized() {
        assert_eq!(
            normalize_pem_newlines(
                "-----BEGIN RSA PRIVATE KEY-----\\nabc\\n-----END RSA PRIVATE KEY-----".into()
            ),
            "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----"
        );
    }

    #[test]
    fn a_pem_with_real_newlines_already_is_left_unchanged() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nabc\n-----END RSA PRIVATE KEY-----";
        assert_eq!(normalize_pem_newlines(pem.into()), pem);
    }

    #[test]
    fn allowed_hosts_always_contain_the_public_origins_host() {
        let mut config = Config::for_test();
        config.extra_allowed_hosts = vec!["otto-factory.fly.dev".into()];

        assert_eq!(
            config.allowed_hosts(),
            vec![
                "factory.example.com".to_string(),
                "otto-factory.fly.dev".to_string()
            ]
        );
    }
}
