//! The scopes otto-factory's resource server understands.
//!
//! Scope strings are only meaningful within one resource server, and the
//! authorization server (otto-auth) no longer has a list compiled into it: each
//! service registers its own at startup (`otto_auth::resources::register`),
//! and the AS validates every authorize, code redemption, refresh, and PAT
//! against that registered row. This module is the single place otto-factory
//! says what it registers, so the registry, the consent-screen copy, and the
//! tools' own scope checks cannot drift apart.
//!
//! An unknown scope is rejected rather than ignored — silently dropping a
//! requested scope gives a client a token that does less than it believes,
//! which fails later and confusingly.

/// What this resource server is called on a consent screen.
pub const RESOURCE_NAME: &str = "otto-factory";

pub const JOBS_READ: &str = "jobs:read";
pub const JOBS_WRITE: &str = "jobs:write";
pub const REPOS_READ: &str = "repos:read";
pub const REPOS_WRITE: &str = "repos:write";
pub const MESSAGES: &str = "messages";
pub const TRACKERS: &str = "trackers";
pub const ORG_ADMIN: &str = "org:admin";

/// Every scope this server will issue.
pub const KNOWN: &[&str] = &[
    JOBS_READ,
    JOBS_WRITE,
    REPOS_READ,
    REPOS_WRITE,
    MESSAGES,
    TRACKERS,
    ORG_ADMIN,
];

/// Granted when a client asks for nothing in particular. Read-only: a client
/// that wants to change anything has to say so, and the user has to see it on
/// the consent screen.
pub const DEFAULT: &[&str] = &[JOBS_READ, REPOS_READ];

#[cfg(test)]
mod tests {
    use super::*;

    /// These are the strings existing tokens and registered clients already
    /// carry. Changing one is a breaking change to every issued grant.
    #[test]
    fn the_scope_list_is_the_one_tokens_already_carry() {
        assert_eq!(
            KNOWN,
            [
                "jobs:read",
                "jobs:write",
                "repos:read",
                "repos:write",
                "messages",
                "trackers",
                "org:admin"
            ]
        );
        assert_eq!(DEFAULT, ["jobs:read", "repos:read"]);
    }

    #[test]
    fn the_defaults_are_read_only_and_known() {
        for s in DEFAULT {
            assert!(KNOWN.contains(s));
            assert!(!s.ends_with(":write") && *s != ORG_ADMIN);
        }
    }
}
