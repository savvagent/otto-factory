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

use std::collections::HashSet;

use crate::error::{Error, Result};
use otto_resource::{PlatformClient, Role};
use otto_tenant::ids::{OrgId, TeamId, UserId};

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

/// Which team-scoped rows a caller may see or touch.
///
/// A row with a null `team_id` is org-wide and open to every member; a row with
/// a `team_id` is open to that team's members and to org owners and admins.
/// Membership is the platform's, so [`TeamScope::for_member`] asks it, and
/// **fails closed**: a platform that cannot answer is an [`Error::Platform`],
/// never a guess and never [`TeamScope::All`]. A team id the platform does not
/// list (unknown, foreign, or deleted) is simply not matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamScope {
    /// Owners and admins: every row.
    All,
    /// Everyone else: org-wide rows, plus rows of exactly these teams.
    Teams(HashSet<TeamId>),
}

impl TeamScope {
    /// The scope of `user`, a member of `org` with `role`.
    ///
    /// Administrators are [`TeamScope::All`] without a platform call. Anyone
    /// else costs one `member_teams` lookup (cached by the client for
    /// `MEMBER_TTL`). A user the platform no longer knows as a member of `org`
    /// is [`Error::AccessRevoked`], not a member of no teams: the token can
    /// outlive the membership by an introspection cache window, and "no teams"
    /// would still open every org-wide row to someone who has left.
    ///
    /// ```no_run
    /// # async fn demo(platform: &otto_resource::PlatformClient, org: otto_tenant::OrgId,
    /// #     user: otto_tenant::UserId) -> of_core::Result<()> {
    /// use of_core::teams::TeamScope;
    ///
    /// let scope = TeamScope::for_member(platform, org, user, otto_resource::Role::Member).await?;
    /// assert!(scope.allows(None)); // org-wide rows are everyone's
    /// # Ok(()) }
    /// ```
    pub async fn for_member(
        platform: &PlatformClient,
        org: OrgId,
        user: UserId,
        role: Role,
    ) -> Result<Self> {
        if role.can_administer() {
            return Ok(Self::All);
        }
        // `Some([])` is a member on no team; `None` is someone the platform
        // does not know in this org at all.
        let teams = platform
            .member_teams(org.as_uuid(), user.as_uuid())
            .await?
            .ok_or(Error::AccessRevoked)?;
        Ok(Self::Teams(
            teams
                .into_iter()
                // The platform scopes the lookup by org already; checking the
                // answer too means a confused response cannot widen the set.
                .filter(|t| t.org_id == org.as_uuid())
                .map(|t| TeamId::from(t.id))
                .collect(),
        ))
    }

    /// Whether a row with this `team_id` is visible.
    pub fn allows(&self, team: Option<TeamId>) -> bool {
        match (self, team) {
            (Self::All, _) | (_, None) => true,
            (Self::Teams(mine), Some(t)) => mine.contains(&t),
        }
    }

    /// The team ids to restrict a query to besides org-wide rows, or `None`
    /// when nothing is restricted.
    pub fn restriction(&self) -> Option<Vec<TeamId>> {
        match self {
            Self::All => None,
            Self::Teams(mine) => Some(mine.iter().copied().collect()),
        }
    }

    /// Whether this scope sees every row (an administrator's).
    pub fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }
}
