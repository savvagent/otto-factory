use of_core::jobs::JobsExt;
use std::str::FromStr;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::HeaderMap;
use axum::Json;
use of_core::jobs::{Status, Tracker};
use of_core::trackers::{
    find_binding_by_external_ref, get_connection, resolve_connection_org, Provider,
};
use of_trackers::sync::InboundDecision;
use of_trackers::webhook::{jira_site_id, verify_and_parse, ParsedWebhook, Verification};
use serde_json::json;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

fn webhook_not_found() -> ApiError {
    ApiError::not_found("no matching webhook")
}

pub async fn receive(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let provider = Provider::from_str(&provider).map_err(|error| {
        tracing::warn!(error = %error, "webhook request named an unknown provider");
        webhook_not_found()
    })?;

    let raw_body = body.as_ref();

    // Both branches converge on the same shape: resolve which org owns the
    // provider-native id (the one unscoped lookup, per spec §5a), then open a
    // single org-pinned `Tx` and do everything else — including, for JIRA,
    // verifying the shared secret — through that one transaction. Never split
    // this into a bootstrap `Tx` plus a second one for the rest; the two
    // reads must observe the same connection row.
    // `warmed` is the org's billing standing, read before the transaction
    // opens (a slow platform never holds a pooled connection) and failing
    // open like `complete_job`'s. It decides only whether a ticket close is
    // billed, never whether it applies.
    let (mut tx, connection, parsed, warmed) = match provider {
        Provider::Github => {
            let secret = state
                .config
                .github_app_webhook_secret
                .as_deref()
                .ok_or_else(|| {
                    // A misconfigured deployment, not an attacker-observable
                    // fact — but a distinct public status here would still
                    // tell an external caller "something about this endpoint
                    // is different from a plain unknown-provider request", so
                    // this answers the same uniform 404 and relies on the
                    // error-level log for operator alerting instead.
                    tracing::error!(
                        provider = %provider,
                        "OF_GITHUB_APP_WEBHOOK_SECRET is not configured; rejecting all GitHub webhooks"
                    );
                    webhook_not_found()
                })?;
            let parsed = verify_and_parse(
                provider,
                &headers,
                query.as_deref(),
                raw_body,
                Verification::Github { secret },
            )
            .map_err(|error| {
                tracing::warn!(provider = %provider, error = %error, "webhook rejected");
                webhook_not_found()
            })?;

            let external_id = parsed.connection_external_id().to_string();
            let org_id = resolve_connection_org(&state.db, provider, &external_id)
                .await
                .map_err(ApiError::from)?
                .ok_or_else(|| {
                    tracing::warn!(provider = %provider, external_id = %external_id, "webhook did not match a registered connection");
                    webhook_not_found()
                })?;

            let warmed = state.meter.warm(org_id).await;
            let mut tx = of_core::platform_events::begin_live(&state.db, org_id, None)
                .await
                .map_err(ApiError::from)?;
            let connection = get_connection(&mut tx, provider)
                .await
                .map_err(ApiError::from)?
                .ok_or_else(|| {
                    // Should be unreachable: `tracker_connection_index` is
                    // only ever written alongside the `tracker_connections`
                    // row it points at (§5a). Treat it as an invariant
                    // violation to alert on, not a distinct public response —
                    // a 500 here would let an external caller distinguish
                    // "unknown id" from "id resolved to a broken row",
                    // narrowing exactly the thing the index is meant to hide.
                    tracing::error!(
                        provider = %provider,
                        org_id = %org_id,
                        "tracker_connection_index resolved an org with no matching tracker_connections row"
                    );
                    webhook_not_found()
                })?;
            (tx, connection, parsed, warmed)
        }
        Provider::Jira => {
            let site_id = jira_site_id(query.as_deref()).map_err(|error| {
                tracing::warn!(provider = %provider, error = %error, "webhook rejected");
                webhook_not_found()
            })?;
            let org_id = resolve_connection_org(&state.db, provider, &site_id)
                .await
                .map_err(ApiError::from)?
                .ok_or_else(|| {
                    tracing::warn!(provider = %provider, external_id = %site_id, "webhook did not match a registered connection");
                    webhook_not_found()
                })?;

            // Before the secret is checked, because that check reads the
            // connection inside the transaction. Bounded and cached per org
            // (see `of_billing::Meter::warm`), so an unverified request naming
            // a known site costs at most one cached status lookup.
            let warmed = state.meter.warm(org_id).await;
            let mut tx = of_core::platform_events::begin_live(&state.db, org_id, None)
                .await
                .map_err(ApiError::from)?;
            let connection = get_connection(&mut tx, provider)
                .await
                .map_err(ApiError::from)?
                .ok_or_else(|| {
                    tracing::error!(
                        provider = %provider,
                        org_id = %org_id,
                        "tracker_connection_index resolved an org with no matching tracker_connections row"
                    );
                    webhook_not_found()
                })?;
            let encoded_secret = connection
                .encrypted_webhook_secret
                .as_deref()
                .ok_or_else(|| {
                    tracing::warn!(provider = %provider, org_id = %org_id, "webhook secret missing for resolved connection");
                    webhook_not_found()
                })?;
            let parsed = verify_and_parse(
                provider,
                &headers,
                query.as_deref(),
                raw_body,
                Verification::Jira {
                    cipher: &state.cipher,
                    encoded_secret,
                },
            )
            .map_err(|error| {
                tracing::warn!(
                    provider = %provider,
                    org_id = %org_id,
                    error = %error,
                    "webhook rejected"
                );
                webhook_not_found()
            })?;
            (tx, connection, parsed, warmed)
        }
    };

    let org_id = connection.org_id;

    match &parsed {
        ParsedWebhook::Event(event) => {
            let binding =
                find_binding_by_external_ref(&mut tx, provider, &event.binding_external_ref)
                    .await
                    .map_err(ApiError::from)?;

            tracing::info!(
                provider = %event.provider,
                org_id = %org_id,
                connection_id = %connection.id,
                external_id = %event.connection_external_id,
                external_ref = %event.binding_external_ref,
                action = %event.action,
                issue_ref = %event.issue.reference,
                binding_id = binding.as_ref().map(|binding| binding.id.to_string()),
                "webhook verified and parsed"
            );

            if binding.is_none() {
                tracing::warn!(
                    provider = %event.provider,
                    org_id = %org_id,
                    external_ref = %event.binding_external_ref,
                    "webhook verified for a connection with no matching repo binding"
                );
            }
            if event.kind == of_trackers::webhook::WebhookEventKind::Issue {
                if let Some(binding) = binding.as_ref() {
                    if binding.connection_id.is_some() {
                        let tracker = match provider {
                            Provider::Github => Tracker::Github,
                            Provider::Jira => Tracker::Jira,
                        };
                        let ticket_ref = of_trackers::sync::ticket_ref(event);
                        let existing = tx
                            .get_job_by_ticket_for_repo(binding.repo_id, tracker, &ticket_ref)
                            .await
                            .map_err(ApiError::from)?;

                        match of_trackers::sync::inbound_decision(event, binding, existing.as_ref())
                        {
                            InboundDecision::Drop | InboundDecision::NoOp => {}
                            InboundDecision::Create {
                                title,
                                description,
                                ticket_ref,
                                remote_revision,
                            } => {
                                tx.create_from_ticket(
                                    binding.repo_id,
                                    tracker,
                                    &ticket_ref,
                                    &title,
                                    description.as_deref(),
                                    remote_revision.as_deref(),
                                )
                                .await
                                .map_err(ApiError::from)?;
                            }
                            InboundDecision::Update {
                                title,
                                description,
                                remote_revision,
                            } => {
                                if let Some(existing) = existing.as_ref() {
                                    tx.update_from_ticket(
                                        &existing.id,
                                        &title,
                                        description.as_deref(),
                                        remote_revision.as_deref(),
                                    )
                                    .await
                                    .map_err(ApiError::from)?;
                                }
                            }
                            InboundDecision::Close {
                                to,
                                result,
                                error,
                                remote_revision,
                            } => {
                                if let Some(existing) = existing.as_ref() {
                                    let closed = tx
                                        .close_from_ticket(
                                            &existing.id,
                                            to,
                                            result.as_deref(),
                                            error.as_deref(),
                                            remote_revision.as_deref(),
                                        )
                                        .await
                                        .map_err(ApiError::from)?;
                                    if to == Status::Completed {
                                        record_inbound_completion(
                                            &mut tx, &closed, &warmed, org_id,
                                        )
                                        .await?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        ParsedWebhook::Ignored(ignored) => {
            tracing::info!(
                provider = %ignored.provider,
                org_id = %org_id,
                connection_id = %connection.id,
                external_id = %ignored.connection_external_id,
                event = %ignored.event,
                "webhook verified and ignored"
            );
        }
    }

    tx.commit().await.map_err(ApiError::from)?;
    Ok(Json(json!({ "ok": true })))
}

/// Record a job completed by its ticket closing, in the transaction that
/// completed it: exactly one completion row (`of_billing::record_job_completed`).
///
/// Billed only when an agent ever claimed the job (`attempts > 0`) **and** the
/// org is not blocked. Claim history, not the status at the close: `attempts`
/// is incremented by every claim and never reset (`repend_job` and an expired
/// claim keep it), so an agent cannot hand a worked job back to `pending` and
/// close its ticket to complete it unbilled. A job never claimed is not
/// billable work, whoever closed its ticket. A blocked org's close is applied
/// (the tracker already closed the ticket, so refusing it would only
/// desynchronise the two) but recorded unbilled, so the budget's hard stop
/// holds on this path as it does for `complete_job`; for a `payment_required`
/// org that makes tracker closes free. A redelivery never reaches here (stale
/// revision or terminal job), so the row is written once.
async fn record_inbound_completion(
    tx: &mut otto_tenant::Tx<'_>,
    closed: &of_core::jobs::Job,
    warmed: &of_billing::Warmed,
    org_id: otto_tenant::ids::OrgId,
) -> ApiResult<()> {
    let claimed = closed.attempts > 0;
    let blocked = warmed.billable_blocked();
    if claimed && blocked {
        tracing::info!(
            org_id = %org_id,
            job_id = %closed.id,
            reason = warmed.blocked_reason().unwrap_or("blocked"),
            "ticket close completed a job for an org whose billable work is blocked; recorded unbilled"
        );
    }
    of_billing::record_job_completed(
        tx,
        None,
        of_billing::CompletionSource::InboundTicketClose,
        claimed && !blocked,
    )
    .await
    // Fail the delivery (rolling the close back) so it can be retried or
    // redelivered, rather than complete a job with no completion row.
    .map_err(|e| ApiError::internal("usage metering", e))?;
    Ok(())
}
