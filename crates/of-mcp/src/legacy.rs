//! Hosts this deployment still answers on but no longer wants to be called by.
//!
//! When the product moved from `otto-factory.savvagent.com` to
//! `otto-tower.savvagent.com`, the old URL had to keep working until a published
//! sunset date, and every response through it had to say so. This module is the
//! whole of that: it decides whether a request arrived through a deprecated
//! host, stamps the response with the notice, and hands the rest of the MCP
//! surface a [`Legacy`] marker so discovery and `initialize` can say the same
//! thing in their own vocabulary.
//!
//! ## The host only selects; it never authorizes
//!
//! The origin host (`X-Forwarded-Host`, else `Host`) is attacker-controlled, so
//! it is used for exactly one thing: choosing among values an operator already
//! configured. It picks which configured legacy host's URLs a discovery document
//! names, and whether the notice is attached. It never reaches token
//! validation: `require_bearer` asks the platform about a token as
//! `OF_RESOURCE_URI` and nothing else, whichever host the request came through,
//! and a test in `of-server` pins that. A caller who forges `X-Forwarded-Host`
//! to name a legacy host gains a deprecation notice and a discovery document
//! naming a URL the operator chose — nothing it could not have read by calling
//! that URL directly.
//!
//! ## Why `X-Forwarded-Host`
//!
//! The production console is a Cloudflare Worker that proxies to the Fly app,
//! and a Worker cannot choose the `Host` of its subrequest: the origin sees its
//! own `fly.dev` name whichever public host the browser or agent used. The
//! Worker therefore *sets* `X-Forwarded-Host` from the request URL, overwriting
//! anything the client sent (`web/worker/index.ts`), and that is the only place
//! the public host survives the hop. A deployment with no proxy has no such
//! header, and `Host` is then the public host itself.
//!
//! ## Why it lives in this crate
//!
//! The headers go on every response, console and webhooks included, so
//! `of-server` applies [`mark`] as its outermost layer. But the only consumers
//! of the [`Legacy`] marker are here — the protected-resource document, the
//! `401` challenge and `initialize` — so the type and the one function that
//! makes it sit beside them.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use chrono::{DateTime, SecondsFormat, Utc};
use http::{HeaderMap, HeaderValue, Uri};

/// The deprecated hosts and what to tell their callers. Built once from
/// configuration; empty (no legacy host) is the normal state outside a rename.
#[derive(Debug, Clone, Default)]
pub struct Deprecation {
    /// Bare hostnames, lowercase, as configured (`OF_LEGACY_HOSTS`).
    hosts: Vec<String>,
    /// The scheme the legacy hosts are served on: the public URL's.
    scheme: String,
    /// The canonical public origin (`OF_PUBLIC_URL`), no trailing slash.
    successor_origin: String,
    /// The canonical MCP endpoint (`OF_RESOURCE_URI`).
    successor_mcp: String,
    sunset: Option<DateTime<Utc>>,
}

impl Deprecation {
    /// `hosts` must already be validated bare hostnames; `of-server`'s config
    /// refuses to start otherwise. They are lowercased here so that matching
    /// cannot depend on how an operator typed them.
    pub fn new(
        public_url: &str,
        resource_uri: &str,
        hosts: impl IntoIterator<Item = impl Into<String>>,
        sunset: Option<DateTime<Utc>>,
    ) -> Self {
        let successor_origin = public_url.trim_end_matches('/').to_string();
        // `OF_PUBLIC_URL` is parsed and refused at startup when it is not an
        // http(s) URL, so the fallback is unreachable from `of-server`; it is
        // here only so a hand-built `Deprecation` cannot panic.
        let scheme = url::Url::parse(&successor_origin)
            .map(|u| u.scheme().to_string())
            .unwrap_or_else(|_| "https".to_string());
        Self {
            hosts: hosts
                .into_iter()
                .map(|h| h.into().to_ascii_lowercase())
                .collect(),
            scheme,
            successor_origin,
            successor_mcp: resource_uri.to_string(),
            sunset,
        }
    }

    /// Whether this request's origin host is one of the deprecated ones, and if
    /// so which. The returned marker carries the *configured* entry, never the
    /// header's spelling, so nothing a caller typed ends up in a response.
    pub fn classify(&self, headers: &HeaderMap, uri: &Uri) -> Option<Legacy> {
        if self.hosts.is_empty() {
            return None;
        }
        let origin = origin_host(headers, uri)?;
        let host = self.hosts.iter().find(|h| **h == origin)?;
        Some(Legacy {
            host: host.clone(),
            origin: format!("{}://{host}", self.scheme),
            successor_origin: self.successor_origin.clone(),
            successor_mcp: self.successor_mcp.clone(),
            sunset: self.sunset,
        })
    }
}

/// A request that arrived through a deprecated host. Placed in the request's
/// extensions by [`mark`]; the MCP transport carries it into `initialize` as
/// part of `http::request::Parts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Legacy {
    /// The configured legacy hostname the request came through.
    pub host: String,
    /// `scheme://host` for that hostname.
    origin: String,
    successor_origin: String,
    successor_mcp: String,
    sunset: Option<DateTime<Utc>>,
}

impl Legacy {
    /// The resource URI a client that started on this host is talking to.
    ///
    /// RFC 9728 §3.3 makes a client check that the metadata's `resource` is the
    /// URL it was given; a fresh OAuth flow started on the old URL would fail
    /// that check if the document named the canonical one. The platform keeps
    /// this URI as an alias of the canonical resource until the sunset, so a
    /// token requested for it is minted for, and introspects as, this service.
    pub fn resource_uri(&self) -> String {
        format!("{}/mcp", self.origin)
    }

    /// The discovery pointer a `401` on this host carries.
    pub fn metadata_url(&self) -> String {
        format!("{}/.well-known/oauth-protected-resource", self.origin)
    }

    /// The paragraph `initialize` instructions open with on this host. Agents
    /// read instructions, not response headers, so this is how the notice
    /// reaches the reader who can act on it.
    pub fn notice(&self) -> String {
        let when = match self.sunset {
            Some(sunset) => format!(
                "It keeps working until {}, and stops working after that.",
                sunset.to_rfc3339_opts(SecondsFormat::Secs, true)
            ),
            None => "It keeps working for now, but it will be retired; the shutdown date \
                     has not been published yet."
                .to_string(),
        };
        format!(
            "DEPRECATED ADDRESS. You reached otto-tower through {old}, its old address. \
             {when} Change this MCP server's URL in your client configuration to {new} \
             and sign in again if your client asks; your jobs, repos and leases are the \
             same on both addresses. Tell the person you are working for, because \
             changing the configuration may be theirs to do.",
            old = self.resource_uri(),
            new = self.successor_mcp,
        )
    }

    /// `Deprecation: true`, a `successor-version` `Link` (RFC 8288) and, with a
    /// sunset date, `Sunset` (RFC 8594). `true` rather than RFC 9745's
    /// `@<timestamp>`: the host was deprecated by an operator's cutover, not on
    /// a date this binary knows. The successor link keeps the request's path so
    /// a client following it lands on the same resource on the canonical host.
    fn stamp(&self, path: &str, headers: &mut HeaderMap) {
        headers.insert("deprecation", HeaderValue::from_static("true"));

        let link = format!(
            "<{}{}>; rel=\"successor-version\"",
            self.successor_origin,
            escape_for_link(path)
        );
        let link = HeaderValue::from_str(&link).or_else(|_| {
            HeaderValue::from_str(&format!(
                "<{}/>; rel=\"successor-version\"",
                self.successor_origin
            ))
        });
        if let Ok(link) = link {
            headers.insert(http::header::LINK, link);
        }

        if let Some(sunset) = self.sunset {
            if let Ok(value) = HeaderValue::from_str(&http_date(sunset)) {
                headers.insert("sunset", value);
            }
        }
    }
}

/// The outermost layer of `of-server`'s router: mark a legacy-host request and
/// stamp its response, whichever surface answers it.
pub async fn mark(
    State(deprecation): State<Arc<Deprecation>>,
    mut req: Request,
    next: Next,
) -> Response {
    let Some(legacy) = deprecation.classify(req.headers(), req.uri()) else {
        return next.run(req).await;
    };
    let path = req.uri().path().to_string();
    req.extensions_mut().insert(legacy.clone());
    let mut response = next.run(req).await;
    legacy.stamp(&path, response.headers_mut());
    response
}

/// The public host a request was addressed to: `X-Forwarded-Host`, else
/// `Host`, else the request target's authority. Lowercase, without a port.
///
/// `X-Forwarded-Host` may be a list when several proxies append to it; the
/// first entry is the one the client used.
pub fn origin_host(headers: &HeaderMap, uri: &Uri) -> Option<String> {
    let from_header = |name| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let authority = from_header("x-forwarded-host")
        .or_else(|| from_header(http::header::HOST.as_str()))
        .or_else(|| uri.authority().map(|a| a.as_str()))?;
    Some(strip_port(authority).to_ascii_lowercase())
}

fn strip_port(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        // An IPv6 literal keeps its brackets so it can never equal a hostname.
        return rest
            .find(']')
            .map_or(authority, |end| &authority[..end + 2]);
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    }
}

/// `<` and `>` would end the link target early; nothing else a request path can
/// legally hold is a problem inside one.
fn escape_for_link(path: &str) -> String {
    path.replace('<', "%3C").replace('>', "%3E")
}

/// An IMF-fixdate (RFC 9110 §5.6.7), the format `Sunset` requires.
pub fn http_date(at: DateTime<Utc>) -> String {
    at.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn deprecation(sunset: Option<DateTime<Utc>>) -> Deprecation {
        Deprecation::new(
            "https://tower.example.com",
            "https://tower.example.com/mcp",
            ["Factory.Example.com"],
            sunset,
        )
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    fn uri() -> Uri {
        "/mcp".parse().unwrap()
    }

    #[test]
    fn the_forwarded_host_wins_over_host() {
        let h = headers(&[
            ("host", "otto-factory-mcp.fly.dev"),
            ("x-forwarded-host", "factory.example.com"),
        ]);
        assert_eq!(
            origin_host(&h, &uri()).as_deref(),
            Some("factory.example.com")
        );
        assert!(deprecation(None).classify(&h, &uri()).is_some());

        // And the other way round: a canonical forwarded host is canonical even
        // when Host happens to name a legacy one.
        let h = headers(&[
            ("host", "factory.example.com"),
            ("x-forwarded-host", "tower.example.com"),
        ]);
        assert!(deprecation(None).classify(&h, &uri()).is_none());
    }

    #[test]
    fn host_is_used_when_nothing_was_forwarded() {
        let h = headers(&[("host", "factory.example.com:8443")]);
        assert!(deprecation(None).classify(&h, &uri()).is_some());
        let h = headers(&[("host", "tower.example.com")]);
        assert!(deprecation(None).classify(&h, &uri()).is_none());
    }

    #[test]
    fn matching_ignores_case_and_port_but_not_suffixes() {
        for legacy in [
            "FACTORY.example.com",
            "factory.example.com:443",
            "factory.example.com, proxy.internal",
        ] {
            let h = headers(&[("x-forwarded-host", legacy)]);
            assert!(deprecation(None).classify(&h, &uri()).is_some(), "{legacy}");
        }
        for other in [
            "evil-factory.example.com",
            "factory.example.com.evil",
            "[::1]:443",
            "",
        ] {
            let h = headers(&[("host", "tower.example.com"), ("x-forwarded-host", other)]);
            assert!(deprecation(None).classify(&h, &uri()).is_none(), "{other}");
        }
    }

    /// The marker names the configured host, never the header's spelling, so
    /// nothing a caller sent is echoed into a discovery document.
    #[test]
    fn the_marker_carries_the_configured_spelling() {
        let h = headers(&[("x-forwarded-host", "FACTORY.EXAMPLE.COM:8443")]);
        let legacy = deprecation(None).classify(&h, &uri()).expect("legacy");
        assert_eq!(legacy.resource_uri(), "https://factory.example.com/mcp");
        assert_eq!(
            legacy.metadata_url(),
            "https://factory.example.com/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn no_configured_host_means_nothing_is_legacy() {
        let none = Deprecation::new(
            "https://tower.example.com",
            "https://tower.example.com/mcp",
            Vec::<String>::new(),
            None,
        );
        let h = headers(&[("x-forwarded-host", "factory.example.com")]);
        assert!(none.classify(&h, &uri()).is_none());
    }

    #[test]
    fn the_stamp_names_the_successor_and_the_sunset_only_when_set() {
        let sunset = Utc.with_ymd_and_hms(2027, 1, 31, 0, 0, 0).unwrap();
        let h = headers(&[("x-forwarded-host", "factory.example.com")]);

        let mut out = HeaderMap::new();
        let legacy = deprecation(Some(sunset)).classify(&h, &uri()).unwrap();
        legacy.stamp("/api/orgs/<x>", &mut out);
        assert_eq!(out["deprecation"], "true");
        assert_eq!(
            out[http::header::LINK],
            r#"<https://tower.example.com/api/orgs/%3Cx%3E>; rel="successor-version""#
        );
        assert_eq!(out["sunset"], "Sun, 31 Jan 2027 00:00:00 GMT");

        let mut out = HeaderMap::new();
        deprecation(None)
            .classify(&h, &uri())
            .unwrap()
            .stamp("/mcp", &mut out);
        assert_eq!(out["deprecation"], "true");
        assert!(out.get("sunset").is_none());
    }

    #[test]
    fn the_notice_names_both_addresses_and_the_date() {
        let sunset = Utc.with_ymd_and_hms(2027, 1, 31, 0, 0, 0).unwrap();
        let h = headers(&[("x-forwarded-host", "factory.example.com")]);
        let notice = deprecation(Some(sunset))
            .classify(&h, &uri())
            .unwrap()
            .notice();
        assert!(notice.starts_with("DEPRECATED"), "{notice}");
        assert!(
            notice.contains("https://factory.example.com/mcp"),
            "{notice}"
        );
        assert!(notice.contains("https://tower.example.com/mcp"), "{notice}");
        assert!(notice.contains("2027-01-31T00:00:00Z"), "{notice}");

        let undated = deprecation(None).classify(&h, &uri()).unwrap().notice();
        assert!(undated.contains("not been published"), "{undated}");

        // Constraint 3: the notice is read by every agent alike.
        let lowered = notice.to_lowercase();
        for client in ["claude", "copilot", "cursor", "codex", "gemini"] {
            assert!(!lowered.contains(client), "{client}");
        }
    }
}
