# Queue filter refetch — make Status/Repo/Team/"only what I queued" actually refetch

## Goal

Fix `savvagent/otto-factory#82`: the queue page's filters update the URL but the job list never
re-fetches, because `setFilter` and the "Clear filters" button use SvelteKit's `replaceState`
(which only updates `history` and the address bar) instead of `goto` (which updates the reactive
`page.url` every filter's `$derived` and the fetch `$effect` depend on).

## Status — 2026-09-09

🚧 In progress.

**Spec:** `docs/specs/2026-09-09-queue-filter-refetch-design.md` — read it first. This plan
implements it exactly.

## Global Constraints

- `web/` conventions: Svelte 5 runes only (`$state`/`$derived`/`$props`/`$effect`), no Svelte 4
  stores, no `export let`. `adapter-static` — no SvelteKit server-side code.
- No AI self-attribution anywhere (commits, comments, docs, PR body).
- Run `cd web && npm run lint -- --write` (prettier) before committing, then re-run `npm run lint`
  to confirm clean.
- This change touches exactly one source file (`web/src/routes/o/[org]/queue/+page.svelte`) — no
  SQL, no MCP tool, no console route, no migration, no config surface. Tenant isolation, metering,
  and public-interface rules do not apply; no cross-org test and no `of-billing::classify` step
  needed.
- Gates: `cd web && npm run check` (svelte-check + tsc), `npm run lint` (prettier), `npm test`
  (vitest — exercises `web/worker/` and the existing render tests; this file has no unit-testable
  seam for the actual defect, see the spec's Assumptions, so this is a vacuous-but-real pass, not a
  skip), `npm run build`.
- No out-of-band artifact beyond the console bundle itself is touched (no `Dockerfile`/`fly.toml`,
  no `web/worker/`, no migration) — state that explicitly in the PR body rather than silently
  omitting the checklist items.

## File Structure

| File                                                    | Responsibility                                                                                     |
| -------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `web/src/routes/o/[org]/queue/+page.svelte`              | **Modify.** Swap `replaceState` for `goto(..., { replaceState: true, keepFocus: true, noScroll: true })` at both call sites; swap the `$app/navigation` import accordingly. |

## Task Order & Rationale

Single task. Both call sites are in the same file, the same import line, and the same fix; there is
no useful intermediate checkpoint between them.

## Task 1 — Replace `replaceState` with `goto` in the queue filters 🚧

**Files:** `web/src/routes/o/[org]/queue/+page.svelte` (modify)
**Interfaces:** Consumes `goto` from `$app/navigation` (replacing the `replaceState` import from the
same module). Produces no new interface — internal event-handler behavior only.

- [ ] **Reproduce the defect first, manually, and record the before-state.** This bug has no
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
- [ ] In `web/src/routes/o/[org]/queue/+page.svelte`, change the import on line 3 from
      `import { replaceState } from '$app/navigation';` to `import { goto } from '$app/navigation';`.
- [ ] Replace the body of `setFilter` (around line 105-110):
      ```js
      function setFilter(key: string, value: string | undefined) {
        const url = new URL(page.url);
        if (value === undefined || value === '') url.searchParams.delete(key);
        else url.searchParams.set(key, value);
        void goto(url, { replaceState: true, keepFocus: true, noScroll: true });
      }
      ```
      (`void` on the call: `goto` returns a `Promise<void>`; the handler doesn't need to await it,
      and an un-awaited promise expression must not read as an accidental omission.)
- [ ] Replace the "Clear filters" button's `onclick` (around line 190-195):
      ```svelte
      onclick={() =>
        goto(new URL(page.url.pathname, location.origin), {
          replaceState: true,
          keepFocus: true,
          noScroll: true
        })}
      ```
- [ ] Add a short comment above `setFilter` naming the failure mode explicitly, so a future edit
      that reaches for `replaceState` again sees the warning inline (per the spec's Risks section):
      a one-line comment stating that `replaceState` from `$app/navigation` does not update the
      reactive `page.url` this page's filters depend on, and that `goto(..., { replaceState: true
      })` is required instead.
- [ ] Repeat the manual browser check from the first step (Status, then Repo, then "only what I
      queued", then "Clear filters") against the same seeded data. Confirm each change narrows or
      restores the table immediately with no reload, and that repeated filter changes do not grow
      `window.history.length` (check via the browser devtools console:
      `history.length` before and after several filter changes should differ by at most 1, not one
      per change).
- [ ] Stop the local `of-server` and leave `.env`/local Postgres state as found (do not commit
      `.env` or any seeded data — both are local-only).
- [ ] Run `cd web && npm run check` — must pass.
- [ ] Run `cd web && npm run lint` — must pass (run `npm run lint -- --write` first if formatting
      drifted, then re-run `npm run lint` to confirm clean).
- [ ] Run `cd web && npm test` — must pass unchanged (vitest over `web/worker/` and existing render
      tests; this task adds no new automated test, per the spec's Assumptions and Risks).
- [ ] Run `cd web && npm run build` — must succeed.
- [ ] Format and commit: from the repo root,
      `git add web/src/routes/o/\[org\]/queue/+page.svelte` and
      `git commit -m "web: refetch the queue when a filter changes"`.
