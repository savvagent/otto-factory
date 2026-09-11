# Otto CLI connect recipe — add the ClientRecipe entry now that the upstream blocker is resolved

## Goal

Add an `otto-cli` `ClientRecipe` to `web/src/lib/clients.ts`, immediately before the `generic`
entry, plus its `docs/clients/matrix.md` row and the six-locale i18n key its `note` needs.

## Status — 2026-09-11

⬜ Not started.

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

## Task Order & Rationale

Single task. The `clients.ts` entry, its i18n key across all six locales, and the matrix row are
one indivisible unit — `npm run check`'s message-completeness gate fails on a partial set, and the
matrix row documents exactly the entry that landed in the same commit.

## Task 1 — Add the Otto CLI ClientRecipe, its i18n key, and the matrix row ⬜

**Files:** `web/src/lib/clients.ts` (modify), `web/messages/{en,es,de,fr,it,hi}.json` (modify),
`docs/clients/matrix.md` (modify)
**Interfaces:** Consumes nothing new. Produces one new `ClientRecipe` object read by
`web/src/routes/o/[org]/connect/+page.svelte` (unmodified — it already iterates `CLIENTS`) and one
new Paraglide message function, `m.client_note_otto_cli()`.

- [ ] In `web/src/lib/clients.ts`, insert the following object into the `CLIENTS` array immediately
      before the `generic` entry (verbatim from the spec's "`ClientRecipe` entry" section):

  ```ts
  {
    id: 'otto-cli',
    label: () => 'Otto CLI',
    kind: 'toml',
    location: '~/.otto/config.toml',
    oauth: (url) =>
      `[[mcp_servers]]\nname = "otto-factory"\ntransport = "http"\nurl = "${url}"\nauth = "oauth"`,
    // `token` is unused on purpose: Otto never accepts the secret as a config field — it is
    // handed over through /mcp's own prompt and stored in the OS keyring instead (see the
    // comment in the rendered snippet below).
    token: (url, _token) =>
      `[[mcp_servers]]\nname = "otto-factory"\ntransport = "http"\nurl = "${url}"\nauth = "bearer"\n\n# then run otto, open /mcp, and paste the token when prompted — Otto stores it in the\n# OS keyring under service "otto", account "mcp:otto-factory"; it is never written to this file.\n# Add/remove and a completed OAuth authorization both require restarting otto to take effect.`,
    note: () => m.client_note_otto_cli()
  },
  ```

  (This is the final, post-quality-review form: `token`'s second parameter is named `_token` with a
  comment recording the omission is deliberate; the TOML comment drops markdown-style backticks
  around `otto` since it is pasted into a real config file; and the `note` message below is worded
  to cover both the OAuth and Token tabs, since `+page.svelte` renders it under whichever is
  selected.)

- [ ] In `web/messages/en.json`, add (after `client_note_generic`, before `nav_organizations`, per
      the file's existing `client_note_*` grouping):
      `"client_note_otto_cli": "Whichever form you use, run \`otto\` and open \`/mcp\` afterward — for OAuth, press \`o\` to authorize in your browser and \`c\` once it redirects back (interactive-only, the same as Claude Code and Copilot CLI above); for a token, paste it when \`/mcp\` prompts for one. Either way, restart otto once: a new or newly-authorized server only connects on the next launch.",`
- [ ] Add the matching translated string at the same key in `web/messages/es.json`,
      `web/messages/de.json`, `web/messages/fr.json`, `web/messages/it.json`, and
      `web/messages/hi.json`, in the same position relative to `client_note_generic` in each file.
      Commands, product names (`Otto`, `Claude Code`, `Copilot CLI`), and the `/mcp`, `o`, `c`
      literals stay verbatim in every translation, per the file's own docstring rule ("Prose is
      translated; anything a machine reads is verbatim") — only the surrounding sentence is
      translated.
- [ ] Run `cd web && npm run check` — this runs `scripts/check-messages.mjs` first; it must report
      no missing key, no dropped placeholder (this message has none), and no plural-category issue
      (this message is not a plural) across all six locales, then svelte-check/tsc must pass clean.
- [ ] In `docs/clients/matrix.md`, add this row to the client table (after the `Codex CLI` row,
      before `Any other MCP client`):
      `| Otto CLI | 0.29.1 | not run | not run | — | Config shape confirmed against upstream source (\`crates/otto/src/config_file.rs\`, \`crates/otto/src/mcp_config_writer.rs\`) and its own round-trip tests; not installed on the conformance machine. |`
- [ ] Manual verification: `cd web && npm run dev` against a running `of-server`
      (`cargo run -p of-server` in another terminal, with `.env` set and `podman compose up -d` for
      Postgres), open the connect page for a real org, select "Otto CLI" in the client picker, and
      visually confirm both the OAuth and token TOML snippets render with the real MCP URL
      substituted in and the note text appears beneath the snippet.
- [ ] Run `cd web && npm run lint` — prettier must accept the formatting of every touched file.
- [ ] Run `cd web && npm test` — vitest (Cloudflare Worker routing tests); expected to pass
      unchanged, confirming this touched nothing it covers (a vacuous pass, not a skip).
- [ ] Run `cd web && npm run build` — confirms the static bundle still builds with the new entry and
      message key compiled in.
- [ ] Format and commit: `cd web && npm run lint -- --write` if needed, then rerun
      `cd web && npm run lint` to confirm it is clean, then from the repo root
      `git add web/src/lib/clients.ts web/messages/en.json web/messages/es.json web/messages/de.json web/messages/fr.json web/messages/it.json web/messages/hi.json docs/clients/matrix.md`
      and `git commit -m "web: add Otto CLI to the connect page's client list"`.
