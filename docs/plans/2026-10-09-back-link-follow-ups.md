# Back-to-otto link: post-merge review follow-ups

Goal: the non-blocking findings from the post-merge reviews of #213 (savvagent/otto-factory#212).

## Status — 2026-10-09

Task 1 ✅.

## Task 1 — Apply the review findings ✅

- [x] Move the link's address decision (session wins, remember through sign-out, discover once
      after the session resolves) into `PlatformHome` (`web/src/lib/platform-home.svelte.ts`)
      and unit-test it, including a discovery that answers after the session does.
- [x] Guard that late answer with `??=`, so it cannot replace an address the session supplied.
- [x] `platformHome` tests for an IPv6 literal, surrounding whitespace, and an uppercase scheme.
- [x] Truncate the header's org name (full name in `title`) so a long one cannot overflow a phone.
- [x] Italian: "Torna alla piattaforma otto" — "otto" alone reads as "eight".
- [x] Cross-reference the two tests that together pin the console's and the discovery
      document's platform address; correct the spec's "only when signed out".
