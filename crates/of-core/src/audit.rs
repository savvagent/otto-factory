//! Audit actions that belong to the factory's own domain.
//!
//! The audit trail itself — `Entry`, `Db::audit`, `Tx::audit`, and the identity
//! and auth action names — is `otto_tenant::audit`. An action is a plain string,
//! so a service adds its own without touching the platform; these are the ones
//! otto-factory writes. Import this module's `action` alongside
//! `otto_tenant::audit::action` under another name where both are needed.

pub mod action {
    // Resources.
    pub const REPO_REGISTERED: &str = "repo.registered";
    pub const REPO_UPDATED: &str = "repo.updated";
    pub const TRACKER_CONNECTED: &str = "tracker.connected";
    pub const TRACKER_DISCONNECTED: &str = "tracker.disconnected";
    pub const TRACKER_BOUND: &str = "tracker.repo.bound";
    pub const TRACKER_UNBOUND: &str = "tracker.repo.unbound";

    // Jobs.
    pub const JOB_CANCEL_REQUESTED: &str = "job.cancel.requested";
    pub const JOB_CANCELLED: &str = "job.cancelled";
}
