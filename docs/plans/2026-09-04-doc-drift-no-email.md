# Doc drift from removing email — cleanup

**Fast-path: no design spec per otto-factory-development trivial-task criteria** — this is a
documentation-only correction with no code or interface change, closing GitHub issue #14.

## Status — 2026-09-04

Complete. All three tasks below landed; both mandatory-trio reviewers on PR #29 (`rust-pro`,
`architect-reviewer`) confirmed their findings were resolved after the review-response
commits.

## Goal

`docs/plans/2026-09-01-milestone-1.md` (Tasks 6, 10, 11, 13),
`docs/specs/2026-09-01-otto-factory-design.md` (§ Layer 2 and the crate table/deployment
sections), and `docs/deploy/cloudflare.md` still describe the TOTP + emailed magic-link
authentication mechanism that `Remove email from the product` (`0009_no_email`) and
`Replace TOTP with passkeys` (`0010_passkeys`) replaced. The task brief also named a stale
comment in `crates/of-auth/src/totp.rs:12` — that file no longer exists (the module was
deleted along with TOTP), so no source change is needed there.

## Task 1 — Correct the drifted docs

- [x] Annotate `docs/plans/2026-09-01-milestone-1.md` Tasks 6, 10, 11, and 13 so each
      passage describing TOTP enrollment, magic-link delivery, `LogMailer`, or the
      emailed-verification signup flow is marked as superseded, with a pointer to the
      commits (`Remove email from the product`, `Replace TOTP with passkeys`) and to
      `CLAUDE.md`'s Authentication section for the mechanism as it stands today. Historical
      narrative (what actually shipped and was verified at the time) is kept, not deleted.
- [x] Update `docs/specs/2026-09-01-otto-factory-design.md`'s "Layer 2 — proving who the
      human is" section, the architecture diagram, the crate responsibility table, the
      deployment secrets line, and the testing bullet to describe passkeys instead of TOTP
      + email.
- [x] Update `docs/deploy/cloudflare.md`'s references to emailed links and magic links to
      reflect the current passkey ceremony, annotating the "Verified locally" section as
      predating the removal.
- [x] Confirm no stale TOTP/magic-link/`LogMailer` comments remain in
      `crates/of-auth/src/`, `crates/of-core/src/`, `crates/of-web/src/`, and
      `crates/of-server/src/`. Found and fixed (beyond the task brief's single
      named file, which no longer exists): stale doc comments in
      `of-auth::lib`, `of-auth::ratelimit`, `of-auth::error`,
      `of-core::audit`, `of-core::crypto`, `of-core::invites` (a dead
      reference to the deleted `of_auth::magic` module), `of-server::config`,
      `of-web::state`, and `of-web::routes::orgs`; a stale
      `reset_member_authenticator` name in both the design spec and
      `CLAUDE.md` (corrected to the actual `reset_member_passkeys`); a stale
      `of-auth` crate-table entry in `CLAUDE.md`; and a `docs/clients/matrix.md`
      re-run snippet that would fail today (`enrol TOTP` — no such step
      exists). All are comment/prose-only; `cargo check --workspace` confirms
      no behavior changed.
- [x] `git commit -m "docs: fix doc drift from removing email (#14)"`.

## Task 2 — Fix additional stale comments found by review

The Rust and architecture reviewers on PR #29 found the sweep in Task 1 was incomplete:
real stale doc comments remained in source outside `docs/`, and the design spec quoted a
function name (`reset_member_authenticator`) that CLAUDE.md itself still had wrong.

- [x] Fix stale TOTP/magic-link/email doc comments in `crates/of-core/src/audit.rs`,
      `crates/of-core/src/crypto.rs`, `crates/of-core/src/invites.rs`,
      `crates/of-server/src/config.rs`, `crates/of-web/src/state.rs`,
      `crates/of-web/src/routes/orgs.rs`.
- [x] Fix `reset_member_authenticator` → `reset_member_passkeys` in both
      `docs/specs/2026-09-01-otto-factory-design.md` and `CLAUDE.md`.
- [x] Fix the stale `of-auth` crate-table entry in `CLAUDE.md` and the misleading
      "passkey/token encryption key" phrasing in the design spec.
- [x] Fix `docs/clients/matrix.md`'s re-run snippet and the milestone plan's "Done means"
      line, both of which described a TOTP enrollment step that no longer exists.
- [x] `cargo check --workspace` — confirms the comment-only changes compile.
- [x] `git commit -m "docs: fix additional stale auth comments found in review (#14)"`.

**Deliberately not fixed here** (flagged as follow-ups, not silently expanded into this
docs-only change): `crates/of-core/src/audit.rs`'s `action::TOTP_ENROLLED` /
`action::TOTP_RESET` constants are still emitted by real passkey-enrollment/reset code
paths, so production audit rows and SIEM exports currently read `auth.totp.enrolled` /
`auth.totp.reset` for passkey events — renaming them is the file's own definition of a
breaking change and needs its own spec. `action::MAGIC_LINK_SENT`,
`action::MAGIC_LINK_CONSUMED`, `action::RECOVERY_CODE_USED`, and `action::EMAIL_VERIFIED`
appear to be dead code (no remaining callers). `of-auth`'s and `of-web`'s `Cargo.toml` both
still depend on `totp-rs`, unused by any remaining source. `totp_issuer` config
(`OF_TOTP_ISSUER`) is plumbed through `of-server` → `of-web` but read by nothing since TOTP
was replaced by passkeys. All three are dependency/config/audit-trail changes, not
documentation, and each needs its own decision about backward compatibility.

No test/lint/build gate applies beyond `cargo check --workspace` — documentation and
comments only, no behavior changed.

## Task 3 — Address automated + human review feedback

- [x] Fix `crates/of-server/src/main.rs`'s stale "first TOTP enrolment" comment, flagged by
      the `rust-pro` reviewer.
- [x] Fix `docs/deploy/cloudflare.md`'s internal inconsistency flagged by
      `copilot-pull-request-reviewer`: the "Verified locally" bullet had dropped "emailed
      verification link" from its historical description while the adjacent parenthetical
      still referenced it.
- [x] `cargo check -p of-server` — confirms the comment-only change compiles.
- [x] `git commit -m "docs: fix stale TOTP comment in of-server main"` /
      `git commit -m "docs: fix cloudflare.md historical narrative inconsistency"`.
