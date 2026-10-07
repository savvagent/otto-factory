//! Team scoping, enforced against the platform.
//!
//! Teams are identity, so they live in the platform's database and this one
//! cannot hold a foreign key to them. What this database stores is an opaque
//! `team_id` on repos, jobs, and messages, where **a null `team_id` means
//! org-wide**. That makes the way a team id gets onto a row the dangerous part:
//! an id that was never checked (a typo, another tenant's team, a team that has
//! since been deleted) must never be written, and a failure to find out must
//! never be read as "no team".
//!
//! [`VerifiedTeam`] is how that is enforced rather than remembered. `NewRepo`,
//! `RepoPatch`, and `NewJob` take one instead of a bare [`TeamId`], and the only
//! way to build one is [`VerifiedTeam::verify`], which asks the platform whether
//! the team exists in the caller's org and **fails closed**:
//!
//! - the platform says the team is not in this org → [`Error::TeamNotFound`];
//! - the platform cannot be reached, or answers with anything unexpected →
//!   [`Error::Platform`]. Never "treat it as org-wide", never "assume it's fine".
//!
//! The reverse direction (a team deleted *after* repos were scoped to it) is
//! handled by the platform's `team.deleted` webhook; see [`crate::platform_events`].

use crate::error::{Error, Result};
use otto_resource::PlatformClient;
use otto_tenant::ids::{OrgId, TeamId};

/// A team id the platform confirmed exists in a particular org.
///
/// Not `Deserialize`, and the fields are private: there is no way to conjure
/// one from untrusted input except by asking the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedTeam {
    org: OrgId,
    team: TeamId,
}

impl VerifiedTeam {
    /// Ask the platform whether `team` exists in `org`.
    ///
    /// ```no_run
    /// # async fn demo(platform: &otto_resource::PlatformClient, org: otto_tenant::OrgId,
    /// #     team: otto_tenant::TeamId) -> of_core::Result<()> {
    /// use of_core::teams::VerifiedTeam;
    ///
    /// // An unknown or deleted team is an error, never an org-wide repo.
    /// let team = VerifiedTeam::verify(platform, org, team).await?;
    /// # let _ = team; Ok(()) }
    /// ```
    pub async fn verify(platform: &PlatformClient, org: OrgId, team: TeamId) -> Result<Self> {
        match platform.team(org.as_uuid(), team.as_uuid()).await? {
            // The platform scopes the lookup by org already; checking the answer
            // too means a confused or compromised response cannot attach a
            // foreign team.
            Some(info) if info.org_id == org.as_uuid() && info.id == team.as_uuid() => {
                Ok(Self { org, team })
            }
            _ => Err(Error::TeamNotFound {
                team: team.to_string(),
            }),
        }
    }

    pub fn id(&self) -> TeamId {
        self.team
    }

    pub fn org(&self) -> OrgId {
        self.org
    }

    /// Refuse a team that was verified for a different org than the
    /// transaction's. A programming error, not a user one, but it is exactly the
    /// cross-tenant mix-up this type exists to make loud.
    pub(crate) fn in_org(self, org: OrgId) -> Result<TeamId> {
        if self.org == org {
            Ok(self.team)
        } else {
            Err(Error::TeamNotFound {
                team: self.team.to_string(),
            })
        }
    }
}
