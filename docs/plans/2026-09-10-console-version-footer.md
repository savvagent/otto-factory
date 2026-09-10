# Console version footer — show the running version in the console

## Goal

Display the product's one SemVer version (workspace crate version / `web/package.json` version,
which move together via release-please) in the console's root footer, so it is visible on every
page — including the pre-login screens — with no server-side change.

## Status — 2026-09-10

🚧 In progress.

**Spec:** `docs/specs/2026-09-10-console-version-footer-design.md` — read it first. This plan
implements it exactly.

## Global Constraints

- `web/` conventions: Svelte 5 runes only (`$state`/`$derived`/`$props`/`$effect`), no `export let`,
  no Svelte 4 stores, Tailwind v4, `adapter-static` (no SvelteKit server).
- No AI self-attribution anywhere (commits, comments, docs).
- Run `npm run lint` (prettier) before committing; there is no `cargo fmt` equivalent needed since
  this task touches no Rust code.
- This change touches only `web/` presentational files — no SQL, no MCP tool, no console API route,
  no migration, no config surface. Tenant isolation, metering, and public-interface rules do not
  apply; no cross-org test and no `of-billing::classify` step are needed.
- No out-of-band artifact beyond the console bundle itself: no `Dockerfile`/`fly.toml` change, no
  `web/worker/` change, no migration. The console bundle (`npm run build`) is the one out-of-band
  surface this touches, and Phase 5's out-of-band verification covers it explicitly.
- Gates: `npm run check` (svelte-check + tsc, includes the new unit test's types), `npm run lint`
  (prettier), `npm test` (vitest — runs the new `version.test.ts` alongside the existing Worker and
  component tests), `npm run build`.

## File Structure

| File                             | Responsibility                                                              |
| --------------------------------- | ---------------------------------------------------------------------------------- |
| `web/src/lib/version.ts`          | **Create.** Exports `APP_VERSION`, read from `web/package.json`'s `version` field. |
| `web/src/lib/version.test.ts`     | **Create.** Unit test: `APP_VERSION` matches `package.json` and looks like SemVer. |
| `web/src/routes/+layout.svelte`   | **Modify.** Footer: add `v{APP_VERSION}` next to the existing `/docs/api` link.    |

## Task Order & Rationale

Single task — the module, its test, and its one call site are tightly coupled and small enough to
land together; there is no intermediate state worth a checkpoint between them.

## Task 1 — Add `APP_VERSION` and show it in the footer ⬜

**Files:** `web/src/lib/version.ts` (new), `web/src/lib/version.test.ts` (new),
`web/src/routes/+layout.svelte` (modify)
**Interfaces:** `version.ts` exports `APP_VERSION: string`, consumed by `+layout.svelte`. No other
module consumes it yet. No server interface of any kind is touched.

- [ ] Confirm the current value: `cd web && node -p "require('./package.json').version"` — expect
      `0.3.0` (matches the workspace `Cargo.toml`'s `version = "0.3.0"` at the time of writing; the
      test below must not hardcode this number, since release-please will move it).
- [ ] Write the failing test first: create `web/src/lib/version.test.ts`:
  ```ts
  import { describe, expect, it } from 'vitest';
  import pkg from '../../package.json';
  import { APP_VERSION } from './version';

  describe('APP_VERSION', () => {
    it('matches package.json', () => {
      expect(APP_VERSION).toBe(pkg.version);
    });

    it('looks like a SemVer version', () => {
      expect(APP_VERSION).toMatch(/^\d+\.\d+\.\d+$/);
    });
  });
  ```
- [ ] Run `cd web && npm test -- version` — expect a failure (`version.ts` does not exist yet /
      `Cannot find module './version'`).
- [ ] Create `web/src/lib/version.ts`:
  ```ts
  import { version } from '../../package.json';

  /**
   * The console bundle's own release version.
   *
   * `web/package.json`'s `version` and the workspace crate version move together
   * via release-please (see CLAUDE.md, "Releases & versioning"), and the console
   * bundle and the `of-server` binary are built in the same `Dockerfile` into one
   * image — so this is exactly the version the accompanying server was built
   * from, with no network call and no possibility of drift between the two.
   */
  export const APP_VERSION = version;
  ```
- [ ] Run `npm test -- version` again — expect both assertions to pass.
- [ ] In `web/src/routes/+layout.svelte`, add the import alongside the existing `$lib` imports
      (near `import { session } from '$lib/session.svelte';`):
      `import { APP_VERSION } from '$lib/version';`
- [ ] Extend the footer (currently just the `/docs/api` link):
  ```svelte
  <footer class="border-t border-edge/40 px-4 py-4 text-center text-xs text-faint">
    <a class="hover:text-muted" href="/docs/api">{m.nav_api_reference()}</a>
    <span aria-hidden="true"> · </span>
    <span>v{APP_VERSION}</span>
  </footer>
  ```
- [ ] Run `cd web && npm run check` — svelte-check + tsc must pass with no new errors (confirms the
      JSON import resolves under `resolveJsonModule` and the new module types cleanly).
- [ ] Run `npm run lint` — prettier must report no issues (run `npm run lint -- --write` first if it
      does, then re-check the diff is what you expect).
- [ ] Run `npm test` — full suite green (confirms the new test coexists with the existing Worker and
      page-render tests, no regressions).
- [ ] Run `npm run build` — confirms the static bundle still builds with the JSON import resolved
      (out-of-band artifact: the console bundle `of-server` serves from `web/build`).
- [ ] Manual visual check: `npm run dev`, load `/login` (ungated) and, once signed in, `/o/<org>`
      (gated) — confirm the footer shows `v<version>` matching `web/package.json`'s `version` field
      on both, next to the existing "API reference" link, and that the `/docs/api` link still
      navigates correctly.
- [ ] Format and commit: no Rust changes, so no `cargo fmt`; run `npm run lint` once more as the
      formatting gate, then
      `git add web/src/lib/version.ts web/src/lib/version.test.ts web/src/routes/+layout.svelte`
      and `git commit -m "web: show the console version in the footer"`.
