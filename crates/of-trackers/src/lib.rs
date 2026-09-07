//! Tracker clients and sync helpers.
//!
//! `of-trackers` owns outbound tracker API calls, webhook verification/parsing,
//! and the later sync engine. This task implements the GitHub App and JIRA
//! OAuth clients only; database access stays in `of-core`.

mod error;
pub mod github;
pub mod jira;
pub mod sync;
pub mod webhook;

pub use error::{Error, Result};

#[cfg(test)]
mod test_support;
