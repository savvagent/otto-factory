# German lease term

Goal: savvagent/otto-factory#183, shipped in #204. The German catalog translated "lease" as
*Sperre* ("lock") — in copy whose point is that a lease is advisory, not a lock. This plan is
recorded after the fact: #204 skipped it so as not to touch `docs/plans/` while #203 was editing
it in parallel.

## Status — 2026-10-08

Task 1 ✅.

## Task 1 — Use the loanword *Lease* ✅

Value-only catalog change; keys and `{repo}` placeholders untouched.

- [x] Choose *das Lease / die Leases* (neuter): already the landing page's term, and matching the
      it/hi catalogs' loanword.
- [x] Replace *Sperre(n)* in every lease string of `web/messages/de.json` (8 keys, including the
      two `error_lease_*` keys the issue did not list), with articles and pronouns re-agreed.
- [x] `npm run check` and `npm run lint` pass.
