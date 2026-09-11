# Otto CLI connect recipe design

> **Status:** APPROVED — unblock `savvagent/otto-factory#41` now that the upstream client ships a
> remote-MCP path, and add its `ClientRecipe` entry to the connect page.

> **Implements:** `savvagent/otto-factory#41`

## Premise corrections

The issue (opened 2026-09-05, parked by its own comment the same day) is titled around "Savvagent
CLI" and its blocker was checked against **savvagent-cli v0.19.3**, where `ToolEndpoint` was
stdio-only and no config, slash command, or plugin effect could add a remote MCP server. Both
halves of that premise have since changed, confirmed by re-fetching the live issue and the
upstream repository during this run (2026-09-11):

- **The upstream project renamed itself.** `github.com/savvagent/savvagent-cli` is now
  `github.com/savvagent/otto` — package name `otto`, binary name `otto`, described in its own
  README as "Otto — fast, MCP-first coding agent CLI." There is no more "Savvagent CLI" to point
  at; the current, correct product name is **Otto CLI**. This spec uses that name throughout, and
  the console entry is named accordingly rather than after the stale identity in the issue title.
- **The blocker is resolved as of `v0.29.1`**, released 2026-09-11 (today), which this run
  confirmed by reading the shipped, tested source rather than release notes:
  - `ToolEndpoint` (`crates/otto-host/src/config.rs`) now has an `Http { name, url, auth: HttpAuth }`
    variant alongside `Stdio`, where `HttpAuth` is `None | Bearer { token } | OAuth { client }`
    built on `rmcp::transport::auth`.
  - A config file — `~/.otto/config.toml`, `[[mcp_servers]]` array of tables — names MCP servers
    directly (`crates/otto/src/config_file.rs`'s `McpServerEntry` enum, mirroring `ToolEndpoint`
    exactly: `Stdio { name, command, args, env }` / `Http { name, url, auth: McpAuthMode }`).
  - A slash command, `/mcp`, opens a manager screen that writes that config
    (`crates/otto/src/plugin/builtin/mcp/mod.rs`, `crates/otto/src/mcp_config_writer.rs`) and drives
    an OAuth ceremony (`crates/otto/src/mcp_oauth.rs`) using PKCE and dynamic client registration —
    the same shape otto-factory's own authorization server implements (RFC 7591 DCR, PKCE S256, a
    loopback redirect).
  - `Effect::RegisterMcpServer` (the plugin-manifest effect the issue cited as explicitly rejected)
    is unrelated to this path — that gate is about *third-party plugins* registering a server on a
    user's behalf, not the user-driven `/mcp` config path this spec targets, which was not gated by
    it at all.
  - This is documented, with unit-tested round-trip coverage of the exact TOML shape, in the
    upstream README's "Adding a new MCP server (user-configured)" section and in
    `crates/otto/src/mcp_config_writer.rs`'s test module — not merely inferred from a design doc.

This spec proceeds with implementation rather than re-parking the issue.

## Goal & Success Criteria

Add **Otto CLI** as a named `ClientRecipe` in `web/src/lib/clients.ts`, immediately before the
`generic` ("Any other MCP client") entry, per constraint 3 (coding-agent agnostic) — closing the
gap the issue identified without inventing a config shape that doesn't exist.

- `CLIENTS` gains an `otto-cli` entry, positioned immediately before `generic`, following the
  file's existing per-entry shape (`id`, `label`, `kind`, optional `location`, `oauth`, `token`,
  optional `note`) with no change to the `ClientRecipe` interface itself.
- The `oauth` and `token` snippets render TOML matching exactly what Otto's own
  `mcp_config_writer::entry_table` produces and what its own tests assert round-trips correctly.
- `docs/clients/matrix.md` gains an Otto CLI row, in the same "config shape confirmed against
  source, not run on this conformance machine" register already used for Cursor and Codex CLI —
  not overstated as a live conformance run this pass did not perform (see Testing).
- `npm run check`, `npm run lint`, and `npm test` all pass in `web/`.

## Scope

**In:**

- One new `ClientRecipe` object in `web/src/lib/clients.ts::CLIENTS`, placed before `generic`.
- Two new i18n message keys (a `note`) across all six locale catalogs
  (`web/messages/{en,es,de,fr,it,hi}.json`), matching the existing `client_note_claude_code` /
  `client_note_copilot` pattern for a client whose OAuth path needs an interactive terminal.
- A new row in `docs/clients/matrix.md`.

**Out:**

- No change to `ClientRecipe`'s interface (`oauth`/`token` stay required, non-optional — Otto CLI's
  `/mcp` screen supports both `bearer` and `oauth`, so — unlike the issue's "option 2" — there is no
  need to bend the shape for a token-only client or add an optional field. This resolves the
  interface question the issue raised as unnecessary rather than answering it the hard way).
- No change to the console's connect-page component (`+page.svelte`) — it already renders whatever
  `CLIENTS` holds; constraint 2 (substrate, not workflow) and the file's own docstring both hold
  that a client's recipe is data, not a bespoke code path.
- No change to `of-mcp`, `of-web`, or any server-side surface — this is a static, client-side data
  table entry; the server already serves Streamable HTTP with Bearer and OAuth 2.1, which is all
  Otto CLI's new `Http` tool-endpoint variant needs.
- No attempt to install, build, or drive the actual `otto` binary's interactive TUI end-to-end in
  this pass — see Testing and Risks for why, and what is verified instead.

## `ClientRecipe` entry

```ts
{
  id: 'otto-cli',
  label: () => 'Otto CLI',
  kind: 'toml',
  location: '~/.otto/config.toml',
  oauth: (url) =>
    `[[mcp_servers]]\nname = "otto-factory"\ntransport = "http"\nurl = "${url}"\nauth = "oauth"`,
  token: (url, token) =>
    `[[mcp_servers]]\nname = "otto-factory"\ntransport = "http"\nurl = "${url}"\nauth = "bearer"\n\n# then run \`otto\`, open /mcp, and paste the token when prompted — Otto stores it in the\n# OS keyring under service "otto", account "mcp:otto-factory"; it is never written to this file.\n# Add/remove and a completed OAuth authorization both require restarting otto to take effect.`,
  note: () => m.client_note_otto_cli()
}
```

Notes on the shape, cited against upstream source:

- `transport`, `name`, `url`, and `auth` are the same fields `mcp_config_writer::entry_table`'s
  `McpServerEntry::Http` arm writes (`crates/otto/src/mcp_config_writer.rs:63-75`), confirmed
  against that module's own `add_server_preserves_malformed_rows_and_comments` test, which asserts
  the identical `transport = "http"` / `url = "..."` / `auth = "bearer"` rendering for a `Bearer`
  entry — field *order* in the snippet above differs from what `entry_table` writes
  (`transport, name, url, auth` upstream vs. `name, transport, url, auth` here), which is inert:
  the format is internally tagged (`#[serde(tag = "transport")]`), so TOML key order carries no
  meaning to the parser.
- **Otto requires a restart to pick up a new or changed `mcp_servers` entry** — added and OAuth
  connections included (upstream README: "Add/remove and successful OAuth authorization still
  require a restart to take effect"). The token snippet's comment says so explicitly, so a reader
  who authorizes and immediately tries calling a tool isn't left wondering why nothing connected.
- `name = "otto-factory"` is the MCP-server *identity* Otto stores this connection under (used to
  key its keyring account, `mcp:otto-factory`) — an arbitrary but stable choice, same role as the
  `otto-factory` key already used in the Copilot CLI and Cursor JSON snippets in this same file.
- The token snippet cannot inline the token into TOML the way Claude Code's command-line snippet
  does: Otto never accepts a secret as a bare config field (`auth = "bearer"` means "load from the
  keyring," per `crates/otto/src/config_file.rs`'s `McpAuthMode` and the upstream README), so the
  token is handed over through `/mcp`'s prompt, not written to the file. The comment says this
  explicitly rather than silently placing the token where the real client would reject it.
- `auth = "oauth"` triggers Otto's own PKCE + dynamic-registration flow inside `/mcp` (press `o` to
  open the browser, `c` after the loopback redirect completes) — interactive, like Claude Code's
  `claude mcp login` and Copilot's TUI-only consent. The `note` says so, mirroring
  `client_note_claude_code` / `client_note_copilot`.

## i18n

New key `client_note_otto_cli`, added to all six locale files
(`web/messages/{en,es,de,fr,it,hi}.json`) per the project's "a new string costs six catalog
entries" rule. English source string:

> "Then run `otto`, open `/mcp`, press `o` to authorize in your browser, and press `c` once it
> redirects back — Otto's OAuth consent is interactive-only, the same as Claude Code and Copilot
> CLI above. Restart otto afterward; a new or newly-authorized server only connects on the next
> launch."

Placeholders: none. Plural category: none (not a plural message). `scripts/check-messages.mjs`
(run by `npm run check`) is the gate that would fail on a missing locale, a dropped placeholder, or
a wrong plural category — translations for the other five locales are added by the implementer,
not machine-stubbed, matching how `client_note_claude_code`/`client_note_copilot` were done.

## `docs/clients/matrix.md` row

Added to the client table:

```
| Otto CLI | 0.29.1 | not run | not run | — | Config shape confirmed against upstream source (`crates/otto/src/config_file.rs`, `crates/otto/src/mcp_config_writer.rs`) and its own round-trip tests; not installed on the conformance machine. |
```

Consistent with the existing Cursor / Codex CLI rows' register — "not installed on the conformance
machine... config shape is in `web/src/lib/clients.ts`" — rather than claiming a live run this pass
did not perform.

## Testing

- `npm run check`, `npm run lint`, `npm test`, `npm run build` in `web/` (the repo's standard gate
  set for any `web/` change).
- `scripts/check-messages.mjs` (invoked by `npm run check`) validates the new message key across
  all six locales.
- Manual: render the connect page locally (`npm run dev` against a running `of-server`), select
  "Otto CLI" in the client picker, and visually confirm the OAuth and token snippets render as
  designed above with a real `of-server` MCP URL substituted in.
- **Not done in this pass, and said so plainly rather than overclaimed:** actually installing Otto
  CLI's interactive TUI and driving `/mcp` against a running `of-server` to observe a live
  connection. Otto ships no headless/scriptable mode for its TUI (confirmed: no `-p`/print flag in
  its README; the only headless entry point is the `otto-host` library's `examples/headless.rs`,
  which requires a real LLM provider and does not exercise the `/mcp` config path at all — it wires
  a `ToolEndpoint` directly in Rust, bypassing the config file and slash command this recipe
  targets). Reproducing the milestone-1 conformance methodology's live-client run (a logging proxy,
  a real signed-in session, a real agent) for a full TUI would need either a PTY-driving harness
  scripted against `otto`'s ratatui screen or a maintainer with the binary already on a machine —
  out of scope for this data-table change, and no worse than the standing treatment of Cursor and
  Codex CLI in the same file today. Flagged as a residual follow-up below, not silently dropped.

## Error Handling & Edge Cases

None beyond the existing per-entry rendering `+page.svelte` already does for every `ClientRecipe` —
this entry introduces no new branch, no new field, no new user input.

## Risks & Open Questions

- **Naming departs from the issue title.** The issue says "Savvagent CLI"; this spec ships "Otto
  CLI" because that is the product's actual current name as of today's upstream release. Flagged
  explicitly for the architect reviewer as a deliberate, documented deviation from the literal issue
  text, justified by the Premise corrections above — not a silent scope change.
- **No live conformance run.** `docs/clients/matrix.md`'s row is honest about this ("not run"), same
  register as Cursor/Codex today. A follow-up issue to actually install and drive Otto CLI (and
  update the matrix row to a real run) is worth filing separately rather than expanding this
  change's scope to build a TUI-automation harness.
- **Upstream is one day old at v0.29.1.** The `Http` variant and `/mcp` flow could still have rough
  edges upstream has not hit yet. This is no different in kind from any other client entry in this
  file (none of the four existing entries are guaranteed stable indefinitely either), so it is not
  a reason to withhold the entry, only a reason the follow-up conformance run matters.
