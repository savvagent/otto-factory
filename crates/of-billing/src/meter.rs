//! Recording tool calls, and refusing the one billable call when the platform
//! says to.
//!
//! ## Two halves, two systems
//!
//! The org's *standing* (its budget, its payment standing, this month's meter
//! totals) lives in the platform, which is the system of record for billing. A
//! call's *record* lives here, in the factory's own database, as a row in
//! `usage_outbox`:
//!
//! - **Recording** is local and atomic. [`Meter::charge`] and
//!   [`Meter::charge_metered`] insert the outbox row in the tool's own
//!   transaction, so a failed call rolls its record back with everything else
//!   (never billed) and a successful one is recorded even if the platform is
//!   down at that instant (never lost). A background shipper
//!   ([`crate::outbox`]) delivers the rows later, exactly once from the
//!   platform's point of view because it dedupes on the row's `event_id`.
//! - **Enforcing** reads the platform's answer to "must this service refuse
//!   billable calls for this org now" ([`UsageStatus::billable_blocked`]),
//!   cached by `PlatformClient::usage_status` for `otto_resource::USAGE_STATUS_TTL`
//!   (a minute). The platform decides; this crate only obeys, and the
//!   platform's spend lags by whatever is still in the outbox plus one cache
//!   window. That bound is the price of the split and is deliberately
//!   accepted.
//!
//! **A change in the console takes up to that minute to apply, in both
//! directions.** An owner who raises the budget or adds a card can still see
//! completions refused until the cached "blocked" answer expires, and an org
//! that has just been blocked can still complete jobs until the cached "not
//! blocked" one does. The refusal says so, so an agent waits a minute rather
//! than concluding the fix did not work.
//!
//! ## Why charging happens *before* the work
//!
//! A charge is made immediately after the tool opens its transaction, before
//! it does anything. That looks wrong for the rule "a failed call is not
//! billed" — and it is exactly what makes the rule true: the outbox row is in
//! the tool's own transaction, so a tool that fails rolls the meter back with
//! it. Charging afterwards would need a second transaction, which can fail on
//! its own, be retried, or be forgotten. Doing it first also puts the refusal
//! where it costs nothing.
//!
//! ## What enforcement means
//!
//! Every tool call is free except a completed job ([`crate::classify`]), so
//! only [`Meter::charge_metered`] can refuse, and only when the platform's
//! status for the org says `billable_blocked()`: its budget's hard stop has
//! tripped (`budget_exceeded`) or it is not in good standing to be billed
//! (`payment_required`). The refusal is [`BillingError::Blocked`]. Free calls
//! are never refused and never look anything up, so a blocked org can still
//! queue, claim, read, and wait — and find out why it is blocked.
//!
//! **If the platform cannot answer, the call is allowed** (and logged). The
//! alternative is refusing paying customers' work because a lookup failed;
//! the call is still recorded in the outbox, so nothing is under-billed.
//! Authentication already requires the platform, so this only matters in the
//! window where a cached token outlives an outage. The decision is remembered
//! per org for [`FAIL_OPEN_FOR`], so an outage costs one bounded lookup per org
//! per few seconds, not one per call.
//!
//! **The lookup happens before the transaction opens.** `complete_job` calls
//! [`Meter::warm`] before it opens its transaction and hands the answer to
//! [`Meter::charge_metered`], which never makes a network call itself. A slow
//! platform therefore never holds a pooled database connection, and the
//! answer the charge judges cannot expire between the two. No other tool
//! calls `warm`: they have nothing to ask.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use otto_resource::{PlatformClient, UsageStatus};
use otto_tenant::ids::{OrgId, UserId};
use otto_tenant::Tx;
use serde::Serialize;

use crate::classify;
use crate::error::{BillingError, Result};
use crate::outbox::{self, Metered};

/// How long the status lookup waits on the platform before allowing the call.
///
/// The lookup runs before the transaction opens, so no connection is held
/// while it waits, but the agent is: the client's own 10 s timeout is far too
/// long to add to a `complete_job`. A cache hit (the normal case) costs
/// nothing and a miss gets this long, after which the call goes ahead and is
/// recorded like any other — the same outcome as the platform being down.
const STATUS_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a failed status lookup is remembered, per org, as "allow".
const FAIL_OPEN_FOR: std::time::Duration = std::time::Duration::from_secs(5);

/// Fraction of the bucket at which a caller starts being warned.
///
/// Eighty percent is early enough that a team has time to do something about it
/// before work starts being refused, and late enough that it is not noise.
pub const WARN_AT: f64 = 0.8;

/// Configuration for the meter.
#[derive(Clone)]
pub struct Meter {
    platform: Arc<PlatformClient>,
    /// Orgs whose last status lookup failed, and until when to skip looking.
    fail_open: Arc<Mutex<HashMap<OrgId, Instant>>>,
    /// Where an owner fixes a block: `OF_UPGRADE_URL`, else the platform
    /// console's home, which lists the user's orgs. Named in the refusal,
    /// because an error that says "raise the budget" without saying where is a
    /// dead end for an agent and an annoyance for a human. Not a per-org link:
    /// the charge knows only the org id, and resolving its slug would put a
    /// platform call inside the transaction.
    pub fix_url: String,
}

impl std::fmt::Debug for Meter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Meter")
            .field("fix_url", &self.fix_url)
            .finish_non_exhaustive()
    }
}

impl Meter {
    pub fn new(platform: Arc<PlatformClient>, fix_url: impl Into<String>) -> Self {
        Self {
            platform,
            fail_open: Arc::default(),
            fix_url: fix_url.into(),
        }
    }

    /// Look the org's standing up now, before the metered tool opens its
    /// transaction, and return it for [`Self::charge_metered`] to judge.
    /// `None` when the platform could not answer in time, which allows the
    /// call (fail open). Never fails: see the module docs.
    pub async fn warm(&self, org: OrgId) -> Option<UsageStatus> {
        self.status_for(org).await
    }

    /// Record one free tool call. Never refuses and never asks the platform:
    /// every tool is free ([`crate::classify`]), and a completed job is
    /// charged through [`Self::charge_metered`] instead.
    ///
    /// The call is recorded in the outbox inside `tx`, so it commits or rolls
    /// back with the tool's own work.
    pub async fn charge(&self, tx: &mut Tx<'_>, user: UserId, tool: &str) -> Result<Charge> {
        let class = classify::classify(tool);
        outbox::enqueue(tx, Some(user), tool, class.is_billable(), None).await?;
        Ok(Charge {
            billable: class.is_billable(),
        })
    }

    /// Charge one metered tool call, or refuse it.
    ///
    /// `status` is what [`Self::warm`] returned for this org before `tx` was
    /// opened; this never asks the platform itself, so nothing here waits on
    /// the network while a connection is held. Refused with
    /// [`BillingError::Blocked`] when `status` is `billable_blocked()`, and
    /// then nothing is written. Allowed when `status` is `None`, the lookup
    /// having failed (fail open; see the module docs). Otherwise the call is
    /// recorded with `metered`'s meter and quantity and `billable = true`,
    /// inside `tx`.
    pub async fn charge_metered(
        &self,
        tx: &mut Tx<'_>,
        user: UserId,
        tool: &str,
        metered: &Metered,
        status: Option<&UsageStatus>,
    ) -> Result<Charge> {
        if let Some(status) = status.filter(|s| s.billable_blocked()) {
            return Err(BillingError::Blocked {
                reason: status.blocked_reason.clone(),
                fix_url: self.fix_url.clone(),
            });
        }
        outbox::enqueue(tx, Some(user), tool, true, Some(metered)).await?;
        Ok(Charge { billable: true })
    }

    /// Record an idempotent replay of `tool` — always as `Free`, regardless
    /// of `tool`'s own classification, and with no status lookup.
    ///
    /// "Both classes are recorded regardless" still has to hold for a replay: it
    /// is a real call that reached the server and was served, just not new work.
    /// Charging it again would double-bill the caller for one logical
    /// job/message; not recording it at all would leave a gap in the history a
    /// future repricing decision needs. Never refused — a replay creates
    /// nothing, so refusing it would refuse a call that is free to serve.
    pub async fn record_replay(&self, tx: &mut Tx<'_>, user: UserId, tool: &str) -> Result<()> {
        outbox::enqueue(tx, Some(user), tool, false, None).await?;
        Ok(())
    }

    /// The platform's usage status for `org`, or `None` if it could not be read
    /// in time (the caller then allows the call). Bounded by
    /// [`STATUS_LOOKUP_TIMEOUT`], and a failure is remembered for
    /// [`FAIL_OPEN_FOR`] so an outage does not add that wait to every call.
    async fn status_for(&self, org: OrgId) -> Option<UsageStatus> {
        if let Some(until) = self
            .fail_open
            .lock()
            .ok()
            .and_then(|m| m.get(&org).copied())
        {
            if until > Instant::now() {
                return None;
            }
        }
        let failure = match tokio::time::timeout(
            STATUS_LOOKUP_TIMEOUT,
            self.platform.usage_status(org.as_uuid()),
        )
        .await
        {
            Ok(Ok(status)) => {
                if let Ok(mut m) = self.fail_open.lock() {
                    m.remove(&org);
                }
                return Some(status);
            }
            Ok(Err(e)) => e.to_string(),
            Err(_) => "timed out".to_string(),
        };
        tracing::warn!(
            org = %org,
            reason = %failure,
            "could not read usage from the platform in time; allowing billable calls for {FAIL_OPEN_FOR:?} (they are still recorded)"
        );
        if let Ok(mut m) = self.fail_open.lock() {
            m.insert(org, Instant::now() + FAIL_OPEN_FOR);
        }
        None
    }

    /// Report an org's standing without charging for anything.
    ///
    /// Used by the `usage` and `whoami` tools, which are themselves free — a
    /// caller must never have to spend an operation to find out how many it has
    /// left. Unlike the status check this surfaces a platform failure, since an
    /// answer was asked for and a made-up one would be worse than an error.
    pub async fn report(&self, org: OrgId) -> Result<Status> {
        let usage = self.platform.usage_status(org.as_uuid()).await?;
        Ok(Status::new(usage))
    }
}

/// `free` -> `Free`.
fn plan_name(plan: &str) -> String {
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Where an org stands against its bucket.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub plan: String,
    /// Billable operations included this month, after any negotiated override.
    pub included_ops: i64,
    pub billable_used: i64,
    /// Never negative: an org past its bucket has none left, not a debt.
    pub remaining: i64,
    /// Every recorded call this month, billable or not.
    pub total_calls: i64,
    pub period_start: chrono::NaiveDate,
    /// True past [`WARN_AT`] of the bucket. Worth surfacing to a human.
    pub warning: bool,
    /// Whether exceeding the bucket stops billable work rather than metering
    /// overage.
    pub hard_stop: bool,
    /// Whether the server refuses billable calls when the platform says the
    /// org is blocked. Always `true`: that is no longer switchable.
    pub enforced: bool,
}

impl Status {
    fn new(usage: UsageStatus) -> Self {
        Self {
            plan: plan_name(&usage.plan),
            included_ops: usage.included_ops,
            billable_used: usage.billable_count,
            remaining: remaining(usage.billable_count, usage.included_ops),
            total_calls: usage.total_count,
            period_start: usage.period_start,
            warning: warning(usage.billable_count, usage.included_ops),
            hard_stop: usage.hard_stop,
            enforced: true,
        }
    }
}

/// The outcome of charging one call.
#[derive(Debug, Clone, PartialEq)]
pub struct Charge {
    pub billable: bool,
}

/// Operations left in the bucket, floored at zero.
fn remaining(used: i64, included: i64) -> i64 {
    included.saturating_sub(used).max(0)
}

/// Whether an org has crossed the warning threshold.
///
/// A bucket of zero counts as exhausted rather than as a division by zero — an
/// org with no included operations is over its limit from the first call.
fn warning(used: i64, included: i64) -> bool {
    if included <= 0 {
        return true;
    }
    used as f64 >= included as f64 * WARN_AT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_never_goes_negative() {
        assert_eq!(remaining(0, 500), 500);
        assert_eq!(remaining(499, 500), 1);
        assert_eq!(remaining(500, 500), 0);
        assert_eq!(
            remaining(900, 500),
            0,
            "an org past its bucket has none left, not a debt"
        );
    }

    /// The boundary customers actually notice: a plan sold as 500 operations
    /// has to deliver 500, not 499.
    #[test]
    fn the_warning_starts_at_four_fifths() {
        assert!(!warning(399, 500));
        assert!(warning(400, 500));
        assert!(warning(500, 500));
    }

    #[test]
    fn an_empty_bucket_is_always_over() {
        assert!(warning(0, 0));
        assert_eq!(remaining(0, 0), 0);
    }

    #[test]
    fn plans_are_shown_capitalised() {
        assert_eq!(plan_name("free"), "Free");
        assert_eq!(plan_name("enterprise"), "Enterprise");
        assert_eq!(plan_name(""), "");
    }
}
