# Otto CLI connect recipe — add the ClientRecipe entry now that the upstream blocker is resolved

## Goal

Add an `otto-cli` `ClientRecipe` to `web/src/lib/clients.ts`, immediately before the `generic`
entry, plus its `docs/clients/matrix.md` row and the six-locale i18n key its `note` needs.

## Status — 2026-09-11

✅ Shipped as `6cb3dd3`. `web/src/lib/clients.ts`'s `otto-cli` entry, the six-locale
`client_note_otto_cli` key, `web/src/lib/clients.test.ts`, and the `docs/clients/matrix.md` row are
all in place (PR #153, including its review-response round — see the spec's revision notes for what
changed there).

**Remaining:** a live conformance run against a real `otto` binary, tracked as
`savvagent/otto-factory#154` — deliberately out of this task's scope (no PTY-driving harness
exists for Otto's TUI), same standing treatment as the Cursor and Codex CLI rows in the same file.

**Spec:** `docs/specs/2026-09-11-otto-cli-connect-recipe-design.md` — read it first. This plan
implements it exactly.

## Global Constraints

- `web/` conventions: `adapter-static` (no SvelteKit server). The MCP URL is a render-time function
  parameter (`(url) => ...`), never hardcoded — consistent with every existing entry.
- No AI self-attribution anywhere (commits, PR body, code comments, docs).
- **Every coding agent gets the same shape** (constraint 3): the new entry follows the exact
  `ClientRecipe` interface every other entry uses — no optional-field bending, no bespoke rendering
  path.
- **A new console string costs six catalog entries.** `client_note_otto_cli` must land in
  `web/messages/{en,es,de,fr,it,hi}.json` in the same run, or `npm run check`'s
  `scripts/check-messages.mjs` gate fails.
- This change touches only `web/src/lib/clients.ts`, `web/messages/*.json`, and
  `docs/clients/matrix.md` — no SQL, no MCP tool, no console route, no migration, no config surface.
  Tenant isolation, metering, and the public-interface rules do not apply; no cross-org test and no
  `of-billing::classify` step needed. Adding an array entry to `CLIENTS` is additive data, not an
  interface change — no breaking-change flag needed (Non-Negotiable Rule 6 does not apply).
- Gates: `cd web && npm run check && npm run lint && npm test && npm run build`.

## File Structure

| File                                | Responsibility                                                        |
| ------------------------------------ | ----------------------------------------------------------------------- |
| `web/src/lib/clients.ts`            | **Modify.** Add the `otto-cli` `ClientRecipe`, before `generic`.       |
| `web/messages/en.json`              | **Modify.** Add `client_note_otto_cli` (source string).                |
| `web/messages/es.json`              | **Modify.** Add `client_note_otto_cli` (Spanish translation).          |
| `web/messages/de.json`              | **Modify.** Add `client_note_otto_cli` (German translation).           |
| `web/messages/fr.json`              | **Modify.** Add `client_note_otto_cli` (French translation).           |
| `web/messages/it.json`              | **Modify.** Add `client_note_otto_cli` (Italian translation).          |
| `web/messages/hi.json`              | **Modify.** Add `client_note_otto_cli` (Hindi translation).            |
| `docs/clients/matrix.md`            | **Modify.** Add the Otto CLI row to the client table.                  |
| `web/src/lib/clients.test.ts`       | **Create.** Added in the review-response round: asserts every `CLIENTS` entry either embeds a token/placeholder in its `token()` snippet or declares a `note`. |
| `web/src/routes/o/[org]/connect/+page.svelte` | **Modify.** Added in the review-response round: gate the "replace the placeholder" warning on whether the rendered snippet actually embeds one, so it isn't shown next to `otto-cli`'s token snippet. |

## Task Order & Rationale

Single task. The `clients.ts` entry, its i18n key across all six locales, and the matrix row are
one indivisible unit — `npm run check`'s message-completeness gate fails on a partial set, and the
matrix row documents exactly the entry that landed in the same commit.

## Task 1 — Add the Otto CLI ClientRecipe, its i18n key, and the matrix row ✅

**Files:** `web/src/lib/clients.ts` (modify), `web/messages/{en,es,de,fr,it,hi}.json` (modify),
`docs/clients/matrix.md` (modify)
**Interfaces:** Consumes nothing new. Produces one new `ClientRecipe` object read by
`web/src/routes/o/[org]/connect/+page.svelte` (unmodified — it already iterates `CLIENTS`) and one
new Paraglide message function, `m.client_note_otto_cli()`.

- [x] In `web/src/lib/clients.ts`, insert the `otto-cli` object into the `CLIENTS` array
      immediately before the `generic` entry. **Superseded by the review-response round** — see the
      spec's "`ClientRecipe` entry" section for the final form: the token snippet's TOML comment
      (three lines of English prose) was removed entirely once `note` was confirmed as the correct,
      translated channel for that guidance, and the guidance itself was corrected to send a token
      user through `/mcp`'s interactive add form rather than describing a paste-then-prompt flow
      that doesn't work (`copilot-pull-request-reviewer` finding on PR #153).
- [x] In `web/messages/en.json`, add `client_note_otto_cli` after `client_note_generic`. **Superseded
      by the review-response round** — see the spec's "i18n" section for the final English string
      and the same correction (add-form guidance, not paste-then-prompt).
- [x] Add the matching translated string at the same key in `web/messages/es.json`,
      `web/messages/de.json`, `web/messages/fr.json`, `web/messages/it.json`, and
      `web/messages/hi.json`, in the same position relative to `client_note_generic` in each file.
      Commands, product names (`Otto`, `Claude Code`, `Copilot CLI`), and the `/mcp`, `o`, `c`, `a`
      literals stay verbatim in every translation, per the file's own docstring rule ("Prose is
      translated; anything a machine reads is verbatim") — only the surrounding sentence is
      translated. Re-translated in the review-response round alongside the English source.
- [x] Run `cd web && npm run check` — reports no missing key, no dropped placeholder (this message
      has none), and no plural-category issue (this message is not a plural) across all six
      locales, then svelte-check/tsc passes clean.
- [x] In `docs/clients/matrix.md`, add the Otto CLI row (after `Codex CLI`, before
      `Any other MCP client`). **Superseded by the review-response round** — the Version column
      uses `—` like the Cursor/Codex rows rather than `0.29.1`, with the version moved into the
      Notes cell instead (architect-reviewer finding), and the Notes cell now also links
      `savvagent/otto-factory#154`, the live-conformance follow-up filed in that same round.
- [ ] Manual verification: `cd web && npm run dev` against a running `of-server`, open the connect
      page for a real org, select "Otto CLI" in the client picker, and visually confirm both the
      OAuth and token TOML snippets render with the real MCP URL substituted in and the note text
      appears beneath the snippet. **Not performed in this pass** — disclosed rather than checked
      off unverified, consistent with this file's own standard for what it has and hasn't actually
      run.
- [x] Run `cd web && npm run lint` — prettier accepts the formatting of every touched file.
- [x] Run `cd web && npm test` — vitest; now includes `web/src/lib/clients.test.ts` (new, added in
      the review-response round), which asserts that any `CLIENTS` entry whose `token()` doesn't
      embed a placeholder/minted token declares a `note` (architect-reviewer finding).
- [x] Run `cd web && npm run build` — confirms the static bundle still builds with the new entry and
      message key compiled in.
- [x] Format and commit: `cd web && npm run lint -- --write` if needed, then rerun
      `cd web && npm run lint` to confirm it is clean, then commit. Done across the initial commit
      (`web: add Otto CLI to the connect page's client list`) and the review-response round's
      follow-up commits on PR #153.
