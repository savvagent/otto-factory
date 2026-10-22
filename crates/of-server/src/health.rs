//! Liveness and readiness, which are not the same question.
//!
//! **`/healthz` never touches the database, on purpose.** A liveness probe
//! answers "is this process wedged, should it be killed?", and an orchestrator
//! kills what fails it. Point one at the database and a thirty-second database
//! blip restarts every replica simultaneously — turning a brief outage into a
//! cold start of the entire fleet, against a database that is already unwell.
//! The one thing worse than a degraded service is a degraded service that keeps
//! restarting.
//!
//! **`/readyz` does touch it**, because it answers a different question: "should
//! traffic come *here*?" The answer to that is no while the database is
//! unreachable, and the remedy is to route elsewhere and try again — not to
//! kill anything. This is the check a load balancer should be pointed at.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use of_billing::outbox::{self, ShipperStatus};
use otto_tenant::Db;

#[derive(Clone)]
struct HealthState {
    db: Db,
    /// `None` where no usage shipper runs beside the router (most tests), so
    /// the probe does not report billing it has no view of.
    shipper: Option<ShipperStatus>,
}

/// How long a readiness check waits on the database.
///
/// Short by design. A readiness probe that hangs for the pool's full acquire
/// timeout is indistinguishable from a hung process to whatever is polling it,
/// and the answer it eventually gives arrives after the decision was needed.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// The paths this module serves. Named so that `of-server` can tell a probe
/// from a console page without repeating them.
pub const PATHS: [&str; 2] = ["/healthz", "/readyz"];

pub fn router(db: Db) -> Router {
    build(db, None)
}

/// [`router`], with `/readyz` also reporting the usage shipper's state.
pub fn router_with_shipper(db: Db, shipper: ShipperStatus) -> Router {
    build(db, Some(shipper))
}

fn build(db: Db, shipper: Option<ShipperStatus>) -> Router {
    let [live, ready] = PATHS;
    Router::new()
        .route(live, get(healthz))
        .route(ready, get(readyz))
        .with_state(HealthState { db, shipper })
}

/// `GET /healthz` — the process is running and the runtime is scheduling.
async fn healthz() -> Response {
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
}

/// `GET /readyz` — this replica can serve a request that needs the database.
///
/// With a usage shipper attached the body also carries a `usage` object:
/// `meters_declared` and `rows_waiting` (the usage outbox's depth, `null` if it
/// could not be counted). `meters_declared: false` with rows waiting is the
/// stall of `docs/deploy/fly.md`: the platform refused the meter declaration, so
/// no usage ships. It never changes the status code. Every replica shares the
/// platform's answer, so failing readiness would take the whole API out of
/// rotation over a billing delay that loses nothing.
async fn readyz(State(HealthState { db, shipper }): State<HealthState>) -> Response {
    // Deliberately *not* `Db::begin`: that needs an `OrgId`, and there is no
    // org here. This is the one place a bare pool query is the right thing,
    // because it touches no tenant table — it is asking about the connection,
    // not about anyone's data.
    let probe =
        tokio::time::timeout(READY_TIMEOUT, sqlx::query("SELECT 1").execute(db.pool())).await;

    match probe {
        Ok(Ok(_)) => {
            let mut body = serde_json::json!({ "status": "ready" });
            if let Some(shipper) = &shipper {
                body["usage"] = usage_detail(&db, shipper).await;
            }
            (StatusCode::OK, Json(body)).into_response()
        }

        // The body says which of the two it was. "not ready" alone sends
        // whoever is paged to look at the wrong thing half the time: a refused
        // connection is a database that is down, a timeout is usually a pool
        // with every connection checked out, and those have different fixes.
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "readiness check failed");
            unready("the database rejected a probe query")
        }
        Err(_) => {
            tracing::warn!(timeout = ?READY_TIMEOUT, "readiness check timed out");
            unready("the database did not answer a probe query in time")
        }
    }
}

async fn usage_detail(db: &Db, shipper: &ShipperStatus) -> serde_json::Value {
    let waiting = tokio::time::timeout(READY_TIMEOUT, outbox::pending(db))
        .await
        .ok()
        .and_then(Result::ok);
    serde_json::json!({
        "meters_declared": shipper.meters_declared(),
        "rows_waiting": waiting,
    })
}

fn unready(reason: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({ "status": "unready", "reason": reason })),
    )
        .into_response()
}
