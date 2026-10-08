//! Billing errors.

pub type Result<T> = std::result::Result<T, BillingError>;

#[derive(Debug, thiserror::Error)]
pub enum BillingError {
    /// The org has spent its included operations and its plan stops rather than
    /// metering overage.
    ///
    /// Written for an LLM caller that has to decide what to do next, so it says
    /// what ran out, how much was included, and where a human can fix it. An
    /// agent that reads only "quota exceeded" will retry; one that reads this
    /// will stop and say something useful to the person it is working for.
    #[error(
        "this organization has used all {included} operations included in its {plan} plan \
         this month, so {tool} was refused. Reads still work — you can still inspect the \
         queue. To queue or claim more work, someone with billing access needs to upgrade \
         at {upgrade_url}."
    )]
    QuotaExceeded {
        tool: String,
        used: i64,
        included: i64,
        plan: String,
        upgrade_url: String,
    },

    /// The platform could not report the org's standing (`whoami`, `usage`).
    /// Never raised by the quota check itself, which fails open — see
    /// [`crate::Meter`].
    #[error("the otto platform could not be reached to read this organization's usage ({0})")]
    Platform(#[from] otto_resource::Error),

    #[error(transparent)]
    Tenant(#[from] otto_tenant::Error),

    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl BillingError {
    /// Stable machine-readable code, for the same reason `of_core::Error` has
    /// one: agents branch on this, humans read the message.
    pub fn code(&self) -> &'static str {
        match self {
            BillingError::QuotaExceeded { .. } => "quota_exceeded",
            BillingError::Platform(_) => "platform_unavailable",
            BillingError::Tenant(e) => e.code(),
            BillingError::Db(_) => "internal_error",
        }
    }

    /// Retrying an exhausted quota does not help until somebody upgrades or the
    /// month rolls over, and an agent that backs off and retries is an agent
    /// burning its own time.
    pub fn retriable(&self) -> bool {
        match self {
            BillingError::QuotaExceeded { .. } => false,
            BillingError::Platform(e) => e.is_retriable(),
            BillingError::Tenant(e) => matches!(e, otto_tenant::Error::Db(_)),
            BillingError::Db(_) => true,
        }
    }
}
