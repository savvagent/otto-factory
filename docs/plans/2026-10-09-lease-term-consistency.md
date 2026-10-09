# Lease term consistency in Spanish and Hindi

Goal: close savvagent/otto-factory#216.

## Status — 2026-10-09

Task 1 ✅.

## Task 1 — Align the lease term ✅

Value-only catalog change; keys and placeholders untouched, `npm run check` and `npm run lint` pass.

- [x] `es.json`: the landing page's *arrendamiento* (`landing_dev_2`, `landing_how_1_body`,
      `landing_how_3_body`) becomes the console's *concesión*.
- [x] `hi.json`: `repos_leases_info_detail` ends "समाप्त हो जाते हैं" (masculine), matching every other
      लीज़ string.
