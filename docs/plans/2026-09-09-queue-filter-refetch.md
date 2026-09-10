# Queue filter refetch — make Status/Repo/Team/"only what I queued" actually refetch

## Goal

Fix `savvagent/otto-factory#82`: the queue page's filters update the URL but the job list never
re-fetches, because `setFilter` and the "Clear filters" button use SvelteKit's `replaceState`
(which only updates `history` and the address bar) instead of `goto` (which updates the reactive
`page.url` every filter's `$derived` and the fetch `$effect` depend on).

## Status — 2026-09-10

✅ Shipped in `savvagent/otto-factory#85`.

**Spec:** `docs/specs/2026-09-09-queue-filter-refetch-design.md` — read it first. This plan
implements it exactly.

## Global Constraints

- `web/` conventions: Svelte 5 runes only (`$state`/`$derived`/`$props`/`$effect`), no Svelte 4
  stores, no `export let`. `adapter-static` — no SvelteKit server-side code.
- No AI self-attribution anywhere (commits, comments, docs, PR body).
- Run `cd web && npm run lint -- --write` (prettier) before committing, then re-run `npm run lint`
  to confirm clean.
- This change touches the production queue page (`web/src/routes/o/[org]/queue/+page.svelte`) plus
  two new test-support files added after review (`web/src/routes/o/[org]/queue/QueueHarness.svelte`,
  `web/src/routes/o/[org]/queue/page.render.test.ts`) — no SQL, no MCP tool, no console route, no
  migration, no config surface. Tenant isolation, metering, and public-interface rules do not apply;
  no cross-org test and no `of-billing::classify` step needed.
- Gates: `cd web && npm run check` (svelte-check + tsc), `npm run lint` (prettier), `npm test`
  (vitest — exercises `web/worker/`, the existing render tests, and (after review) a new
  `o/[org]/queue/page.render.test.ts` that mocks `$app/navigation` to assert `goto`, not
  `replaceState`, is called by the filter controls — see the spec's Assumptions), `npm run build`.
- No out-of-band artifact beyond the console bundle itself is touched (no `Dockerfile`/`fly.toml`,
  no `web/worker/`, no migration) — state that explicitly in the PR body rather than silently
  omitting the checklist items.

## File Structure

| File                                                    | Responsibility                                                                                     |
| -------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `web/src/routes/o/[org]/queue/+page.svelte`              | **Modify.** Swap `replaceState` for `goto(..., { replaceState: true, keepFocus: true, noScroll: true })` at both call sites; swap the `$app/navigation` import accordingly. |
| `web/src/routes/o/[org]/queue/QueueHarness.svelte`        | **Add** (post-review). Test harness for rendering the queue page in isolation. |
| `web/src/routes/o/[org]/queue/page.render.test.ts`        | **Add** (post-review). Asserts `goto`, not `replaceState`, is called by the filter controls. |

## Task Order & Rationale

Single task. Both call sites are in the same file, the same import line, and the same fix; there is
no useful intermediate checkpoint between them.

## Task 1 — Replace `replaceState` with `goto` in the queue filters ✅

**Files:** `web/src/routes/o/[org]/queue/+page.svelte` (modify)
**Interfaces:** Consumes `goto` from `$app/navigation` (replacing the `replaceState` import from the
same module). Produces no new interface — internal event-handler behavior only.

- [x] **Reproduce the defect first, manually, and record the before-state.** This bug has no
      automated seam (see spec Assumptions), so the "failing test" for this task is a scripted manual
      browser check, run once before the fix and once after, not a new test file:
      1. `podman compose up -d` (Postgres 16 on host port 15433); confirm `.env` has `DATABASE_URL`,
         `OF_PUBLIC_URL`, `OF_ENCRYPTION_KEY` set (`cp .env.example .env` if missing, generating a key
         with `openssl rand -base64 32`).
      2. `cd web && npm run build` (produces `web/build`, which `of-server` serves).
      3. From the repo root, `cargo run -p of-server` (reads `.env`; binds `:8080` by default).
      4. In a browser, sign up (passkey ceremony — a platform authenticator, Touch ID/Windows Hello,
         or a browser's virtual-authenticator devtools panel), create an org, and either seed a couple
         of jobs across two repos/two statuses directly in Postgres for a fast check, or register a
         real repo and queue jobs over MCP from a connected agent.
      5. On the Queue page, select a Status value. Confirm (pre-fix) the URL changes to `?status=...`
         and the `<select>` shows the new value, but the table keeps every job and the "Clear
         filters" link never appears. This is the failing state the task fixes.
- [x] In `web/src/routes/o/[org]/queue/+page.svelte`, change the import on line 3 from
      `import { replaceState } from '$app/navigation';` to `import { goto } from '$app/navigation';`.
- [x] **Update the file's top-of-component doc comment (lines 27-29)**, which currently reads:
      ```
      * Filters live in the URL so a view can be linked to. `replaceState` rather
      * than `goto`: changing a filter is not a place in history to go back to, and
      * a dozen entries per session makes the browser's back button useless.
      ```
      This is the exact rationale the fix overturns — left as-is, it flatly contradicts the code
      right below it and is the comment most likely to mislead a future editor into reverting to
      bare `replaceState`. Replace it with something that states the corrected fact: filters live
      in the URL so a view can be linked to, and `goto(..., { replaceState: true })` (not bare
      `replaceState` from `$app/navigation`, which never updates the reactive `page.url` this page's
      filters read) is what applies a filter without adding a history entry per change.
- [x] Replace the body of `setFilter` (around line 105-110):
      ```js
      function setFilter(key: string, value: string | undefined) {
        const url = new URL(page.url);
        if (value === undefined || value === '') url.searchParams.delete(key);
        else url.searchParams.set(key, value);
        // `page.state` is intentionally not passed through: this page never calls
        // `pushState` or otherwise sets custom page state, so there is nothing to carry.
        void goto(url, { replaceState: true, keepFocus: true, noScroll: true });
      }
      ```
      (`void` on the call: `goto` returns a `Promise<void>`; the handler doesn't need to await it,
      and an un-awaited promise expression must not read as an accidental omission.)
- [x] Replace the "Clear filters" button's `onclick` (around line 190-195):
      ```svelte
      onclick={() =>
        void goto(new URL(page.url.pathname, location.origin), {
          replaceState: true,
          keepFocus: true,
          noScroll: true
        })}
      ```
      (Same `void` prefix as `setFilter`, for consistency between the two call sites this task
      touches — the codebase uses both `await goto(...)` and `void goto(...)` elsewhere, so either
      is acceptable, but the two sites in this file should match each other.)
- [x] Repeat the manual browser check from the first step (Status, then Repo, then "only what I
      queued", then "Clear filters") against the same seeded data. Confirm each change narrows or
      restores the table immediately with no reload, and that repeated filter changes do not grow
      `window.history.length` (check via the browser devtools console:
      `history.length` before and after several filter changes should differ by at most 1, not one
      per change).
- [x] Stop the local `of-server` and leave `.env`/local Postgres state as found (do not commit
      `.env` or any seeded data — both are local-only).
- [x] Run `cd web && npm run check` — must pass.
- [x] Run `cd web && npm run lint` — must pass (run `npm run lint -- --write` first if formatting
      drifted, then re-run `npm run lint` to confirm clean).
- [x] Run `cd web && npm test` — must pass unchanged (vitest over `web/worker/` and existing render
      tests; this task adds no new automated test, per the spec's Assumptions and Risks).
- [x] Run `cd web && npm run build` — must succeed.
- [x] Format and commit: from the repo root,
      `git add web/src/routes/o/\[org\]/queue/+page.svelte` and
      `git commit -m "web: refetch the queue when a filter changes"`.

**Manual verification record.** Performed via a local `of-server` (Postgres on `:15433`, `web/`
built) and a real browser (a resident-key CDP virtual WebAuthn authenticator for the passkey
ceremony), with 4 jobs seeded across 2 repos and 4 statuses. Pre-fix: selecting Status="Pending"
updated the URL to `?status=pending` and the `<select>`, but the table kept all 4 jobs and no "Clear
filters" link appeared — the exact documented defect. Post-fix: Status="Pending" narrowed the table
to 1 row immediately; adding Repo further narrowed it (including to "No jobs match these filters");
toggling "only what I queued" updated correctly; "Clear filters" restored all 4 rows and the clean
URL. `window.history.length` stayed constant across all 4 filter changes, confirming no history-entry
growth. Seeded data and the test account were removed from the shared dev database afterward.
