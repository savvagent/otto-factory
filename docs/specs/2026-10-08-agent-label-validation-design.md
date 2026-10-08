# Agent label validation design

> **Status:** DRAFT — bound and validate the caller-chosen `agent` label on `claim_jobs`,
> `acquire_lease`, and `send_message` at write time, render it quoted wherever it is
> interpolated into error prose a peer agent reads, and withhold a non-conforming legacy label
> from every other output. Closes savvagent/otto-factory#163. See the Addendum for what changed
> after review.

> **Follows from:** `docs/specs/2026-09-11-claim-generation-fencing-design.md` (shipped in
> savvagent/otto-factory#161), whose round-2 security review flagged this and deferred it here.

## Addendum: review round 1 (PR #208)

The mandatory review trio changed four decisions below. Where the body of this spec disagrees
with this section, this section wins.

1. **`send_message` is in scope** (independent security review, Medium). The Out-list argument
   below — "bounding the label beside a body closes nothing" — missed that the body is capped at
   `MAX_BODY_LEN` while the label had **no cap at all**: an unbounded string stored in the shared
   `messages` table and delivered to every recipient is a storage and response-size problem as
   well as the same injection surface. `MessagesExt::send_message` and `find_replayed_message`
   now run `agent_label::validate` before any SQL, and the stored label is the normalized one.
   The idempotency fingerprint still hashes the label *as sent*, so a replay of the identical
   call matches its own fingerprint and a message stored before this change still replays
   (`a_keyed_send_message_with_a_padded_label_replays_cleanly`).
2. **Legacy labels are withheld from structured output too** (security review, Medium). The
   label also reaches peers as a JSON field (`claimedByLabel`, `holderLabel`, `senderLabel`) of
   `get_job` / `list_jobs` / `ready` / `list_leases` / `read_messages` and the console API.
   Rather than a migration (the per-org loop `CLAUDE.md` requires, for no benefit a read-side
   filter cannot give), each field carries `#[serde(serialize_with =
   "agent_label::serialize_stored")]`, which emits `agent_label::displayable(label)` — the trimmed
   label if it passes today's rule, else `null`. `sync_ticket`'s manual tracker write-back uses
   `displayable` the same way. A side benefit: tightening the rule later automatically withholds
   labels the new rule refuses.
3. **Quoting is explicit, not `{:?}`** (Rust review, Important). `Debug` for `str` is not a
   stable format and escapes combining marks, so `cafe\u{301}` or a Hindi label would render as
   `\u{...}` noise. `holder` now writes the quotes itself and escapes only `"` and `\`.
4. **The deny-list is wider** (Rust and security reviews). It now also refuses every whitespace
   character except the plain space (U+00A0, U+2000–U+200A, U+3000, …), the remaining format
   characters (U+0600–U+0605, U+06DD, U+070F, U+0890–U+0891, U+08E2, U+FFF9–U+FFFB, U+110BD,
   U+110CD, U+13430–U+1343F, U+1BCA0–U+1BCA3, U+1D173–U+1D17A), U+2800 (braille blank), and the
   private-use areas (U+E000–U+F8FF, U+F0000–U+10FFFF). Unassigned code points are still allowed:
   detecting them needs a Unicode version table this crate does not carry, and quoting is the
   backstop. The refusal's "at character N" now says it counts after trimming.

5. **Only the plain space is trimmed** (Copilot review). `str::trim` strips all Unicode
   whitespace, so `"ci-7\n"`, `"\tci-7"`, or a bare U+2028 used to be accepted as `"ci-7"` or no
   label, contradicting §1's "any control or line-break character is refused". `validate` now
   trims only U+0020, so a forbidden character at either edge is refused like one in the middle,
   and a label made only of plain spaces is the only blank that means "no label". U+FFFC (object
   replacement) joins the deny-list in the same round.

Also from review: `claim_jobs` makes a single `sync_jobs_after_transition` call, passing the label
stored on the first claimed job, instead of a per-job loop. A test pins the "128" written in the
three tool descriptions to `MAX_LEN`.

**Considered, not done:**
- A `maxLength` in the input schema. The limit applies *after* trimming, so a strict client
  enforcing `maxLength: 128` would refuse a padded label the server accepts. The description
  states the limit instead.
- Rendering the label as inline code in the GitHub/JIRA claim comment (`of-trackers`'
  `Claimed by {agent}.`), so that `@mention` and markdown in a label stop rendering. The comment
  leaves the org, but it is posted by the org's own tracker app into the org's own tickets. It
  already carries free text from `result`, `error`, and cancellation notes with the same
  property, so it belongs with those, not here. See Risks.

**Public interface accounting, revised:** the break now covers three tools (`send_message`
too), plus one visible data change: a blank `agent` used to be stored and returned as `""` and
now comes back as `null`. A legacy non-conforming label also now comes back as `null` in
structured output.

## Premise corrections

The issue names `claimed_by_label` alone. Reading the code, the same shape exists twice:

| Stored column | Written from | Interpolated into error prose at |
| --- | --- | --- |
| `jobs.claimed_by_label` | `claim_jobs`'s `agent` argument | `ensure_claim_held` (`crates/of-core/src/jobs.rs`), both `Error::AlreadyClaimed` "is currently claimed by {holder}" messages — returned to *another* caller of `complete_job` / `fail_job` / `cancel_job` / `renew_claim` |
| `repo_leases.holder_label` | `acquire_lease`'s `agent` argument | `Error::LeaseHeld`'s "{resource} of this repo is leased by {holder} until …" (`crates/of-core/src/leases.rs`) — returned to *another* caller of `acquire_lease` |

Both are the same argument name (`agent`), the same documented meaning ("how you want to be
identified to teammates", example `"api-agent@ci-7"`), and the same exposure: text one org
member chose, rendered inside a server-authored sentence that a different member's agent reads
as tool output. Fixing one and not the other would leave the identical surface open one tool
over, so both are in scope.

## Goal & Success Criteria

An agent label is short, single-line, visible text, and wherever the server quotes one back in
an error sentence it is visibly delimited as a quoted value rather than blended into the
server's own prose.

- `claim_jobs` and `acquire_lease` refuse an `agent` longer than 128 characters, or containing a
  control character, a line or paragraph separator, or an invisible formatting character, with
  the stable code `invalid_agent_label`. Nothing is written and nothing is billed.
- The refusal never echoes the offending label back.
- A blank or whitespace-only `agent` is the same as omitting it; surrounding whitespace is
  trimmed before storage.
- `AlreadyClaimed` and `LeaseHeld` render the holder as `agent "<label>"` (explicit quotes, with `"` and `\` escaped)
  when a label is stored, and as `user <uuid>` otherwise — including for a legacy row written
  before this change whose label would now fail validation.
- Both tool descriptions (and the `agent` argument's schema description) state the rule.

## Scope

**In:**

- `of_core::agent_label` — one module holding the policy (`validate`) and the rendering
  (`holder`), so both call sites and both error messages share one definition.
- `JobsExt::claim_jobs` and `LeasesExt::acquire_lease` validate their `label` argument before
  any SQL.
- `ensure_claim_held` and `acquire_lease`'s `LeaseHeld` branch render the holder through
  `agent_label::holder`.
- New `Error::InvalidAgentLabel` variant, code `invalid_agent_label`, not retriable.
- `claim_jobs` and `acquire_lease` tool descriptions and `agent` field docs in `of-mcp`, and
  `claim_jobs`'s tracker write-back using the stored label (§4).

**Out:**

- ~~**`send_message`'s `agent` (`messages.sender_label`).**~~ Moved into scope by the
  Addendum (item 1).
- **Repo slugs and names in `Error::RepoUnresolved`'s "Registered repos: …" list.** Same
  pattern (peer-chosen text in error prose), but a different input (`register_repo`, gated by
  `repos:write`), with its own compatibility question (existing slugs are identifiers that
  agents pass back, so refusing or re-rendering them is not a label policy). Filed as
  savvagent/otto-factory#206 per the skill's "same bug pattern elsewhere" rule rather than
  widening this change.
- **The caller's own input echoed back** (`LeaseHeld`'s `resource`, `TicketAlreadyLinked`'s
  `ticket_ref`): these repeat what *this* caller just sent, so they cannot carry another
  member's text.
- **Rewriting stored legacy labels.** No migration. A migration that rewrites tenant rows needs
  the per-org loop `CLAUDE.md` describes, and the benefit is nil: render-time handling (§3, and
  the Addendum's item 2 for structured output) already withholds a non-conforming legacy label
  everywhere it would leave the server.
- **Validating against a list of known clients** — constraint 3 forbids it. The rule is purely
  about shape, never about which agent it names.
- Any change to tracker sync beyond §4's one line (which makes the ticket comment carry the
  stored, normalized label rather than the raw argument).

## §1 — The policy

`of_core::agent_label::validate(label: Option<&str>) -> Result<Option<&str>>`:

1. `None` → `Ok(None)`.
2. Trim surrounding whitespace (`str::trim`). Empty → `Ok(None)`. This matches
   `acquire_lease`'s existing rule in `of-mcp` that a blank string and an absent field are the
   same request, and keeps any client that sends `""` for "no label" working.
3. More than `MAX_LEN = 128` characters (Unicode scalar values, `chars().count()`, not bytes —
   a limit stated in characters is the one an LLM caller can reason about, and 128 scalar
   values is at most 512 bytes) → refuse.
4. Any character that is `char::is_control()` (Unicode Cc: C0 — including `\n`, `\r`, `\t`,
   NUL — DEL, and C1), U+2028 / U+2029 (line and paragraph separators), or in the invisible
   formatting set — U+00AD, U+034F, U+061C, U+115F, U+1160, U+17B4, U+17B5, U+180B–U+180F,
   U+200B–U+200F, U+202A–U+202E, U+2060–U+206F, U+3164, U+FE00–U+FE0F, U+FEFF, U+FFA0,
   U+FFF0–U+FFF8, U+E0000–U+E0FFF (tags and variation selectors) → refuse.
5. Otherwise `Ok(Some(trimmed))`.

**Refuse, not truncate.** Truncation is a silent rewrite: the caller would believe teammates see
the label it sent, and a cut can land mid-word in a way that changes meaning. `CLAUDE.md`'s
"no silent fallbacks" rule applies. An explicit refusal costs a well-behaved caller one retry
with a shorter label, once.

**Why 128.** The documented example is 14 characters; realistic composite labels
(`claude-code@host/worktree-<16 hex>`) run to roughly 60. 128 leaves twice that headroom while
cutting the room available for a planted paragraph from unbounded to one short sentence.

**Why these characters and not an allow-list.** An ASCII or `[A-Za-z0-9@._/-]` allow-list would
be simpler but would refuse legitimate non-English labels and spaces, and would read as an
opinion about what an agent's name should look like. The deny-list targets exactly the
characters that let a label *look* like something other than one short line: line breaks (a
label that starts a new paragraph of the error), other control characters (terminal and parser
confusion), and zero-width / bidi / tag characters (text that renders differently from what an
LLM tokenizes, or hides content entirely). Everything else visible is allowed.

**What this does not do.** 128 printable characters can still say "ignore previous instructions".
The policy bounds and delimits the surface; it does not and cannot detect intent. §3's quoting is
the other half: the label arrives inside quotes, after the word `agent`, so it reads as a value
the server is reporting rather than as the server speaking. The residual risk is intra-tenant
only (Tx/RLS scope every read to one org) and is accepted.

## §2 — The error

New variant in `crates/of-core/src/error.rs`:

```rust
#[error(
    "the agent label {problem}. Pass agent as a single line of at most 128 visible \
     characters with no control, line-break, or invisible formatting characters (for \
     example \"api-agent@ci-7\"), or omit it; nothing was changed."
)]
InvalidAgentLabel { problem: String },
```

`code()` → `"invalid_agent_label"`, `retriable()` → `false` (the identical call can never
succeed). `problem` is built by `validate` and never contains the label itself — only its length
(`"is 300 characters long"`) or the offending code point and its 1-based position
(`"contains U+000A at character 12"`). Echoing the label would hand the very text being refused
straight back as tool output.

A dedicated variant rather than `Error::Invalid` (`invalid_argument`) gives agents a stable branch
point that says exactly which argument to fix.

## §3 — Rendering a holder

`of_core::agent_label::holder(label: Option<&str>, user: UserId) -> String`:

- If `label` passes `validate` and is `Some(l)` → `agent "<l>"`, e.g. `agent "api-agent@ci-7"`.
  The function writes the delimiting quotes itself and escapes any `"` or `\` inside (Addendum
  item 3), so a label cannot close its own quotes.
- Otherwise (no label, or a legacy label that fails today's rule) → `format!("user {user}")`.

This is a rendering choice, not a resolution fallback: the user id is the authoritative holder,
already returned as structured data (`claimedBy`, `holderUserId`); the label was only ever a
friendlier name for it.

Call sites:

- `ensure_claim_held`: `holder` becomes `agent_label::holder(claimed_by_label.as_deref(), u)`
  when `claimed_by` is `Some(u)`; the existing data-inconsistency string stays for the
  unreachable `claimed_by IS NULL` case.
- `acquire_lease`'s `LeaseHeld`: `holder: agent_label::holder(live.holder_label.as_deref(),
  live.holder_user_id)`.

Before → after, for an `AlreadyClaimed` refusal: `job job-7 is currently claimed by
api-agent@ci-7, not you — …` → `job job-7 is currently claimed by agent "api-agent@ci-7", not
you — …`.

## §4 — Tool surface (`crates/of-mcp`)

- `ClaimJobsArgs::agent` and `AcquireLeaseArgs::agent` field docs (which become the input
  schema's descriptions): replace "Free-form." with "At most 128 characters on one line, with no
  control or invisible formatting characters; otherwise the call is refused with
  invalid_agent_label. Blank is the same as omitting it."
- `claim_jobs` and `acquire_lease` tool descriptions gain one sentence: "agent is a short label
  (one line, at most 128 visible characters) shown to teammates; a longer or multi-line one is
  refused with invalid_agent_label rather than shortened."

The validation lives in `of-core`, where both write paths already go, and the refusal rolls back
the transaction — including the `usage_outbox` row `Factory::charge` wrote — so a refused call is
never billed. One handler line changes: `claim_jobs` currently hands the *raw* `args.agent` to
`sync_jobs_after_transition`, which posts `Claimed by {agent}.` to the linked GitHub/JIRA ticket.
It passes the stored, normalized `claimed_by_label` of the claimed jobs instead (every job in one
claim carries the same label), the way `sync_ticket` already does — otherwise a blank label
(stored NULL) would still post `Claimed by    .` and a padded one would post untrimmed, on the one
surface that leaves the org.

## Tenant isolation

No new table, no new SQL, no policy change. The two validated writes and two rendered errors are
inside existing `Tx` methods that already carry `org_id = $1`. The surface being closed is
intra-tenant by construction.

## Metering

No new tool. A refused `claim_jobs` / `acquire_lease` fails inside its own transaction after
`Factory::charge`, so its outbox row rolls back with it (rule 1 in `CLAUDE.md`'s Metering
section).

## Public interface note — breaking

This **narrows accepted input** on two MCP tools: a call that previously succeeded with a
>128-character or multi-line `agent` is now refused, and the wording of two existing error
messages changes (`agent "…"` / `user <uuid>` instead of the bare label). No tool, field,
envelope, route, env var, or migration is added, renamed, or removed, and the error codes of the
existing messages are unchanged.

A caller that worked yesterday can fail today, with no opt-out, so under Non-Negotiable Rule 6
this is treated as **breaking**: the PR title carries `!` (`fix(of-core)!: …`) and a
`BREAKING CHANGE:` footer, matching the precedent of savvagent/otto-factory#161, whose
unconditional expiry check was likewise a refusal of previously accepted calls. With
`bump-minor-pre-major`, release-please cuts a minor version for it. No coding-agent client is
known to send such a label, so `docs/clients/matrix.md` (which records what clients send at the
wire level) needs no entry.

## Error Handling & Edge Cases

- `agent: "  api-agent  "` → stored as `"api-agent"`.
- `agent: ""` / `"   "` → stored as `NULL`; the error renders `user <uuid>`.
- Exactly 128 characters → accepted; 129 → refused.
- 128 multi-byte characters (e.g. 128 × `é`) → accepted; the limit is characters, not bytes.
- `\t` inside → refused (it is Cc), even though it is "whitespace": a tab is not visible as such
  in rendered prose.
- A `"` inside a valid label → accepted, rendered escaped as `\"` inside the quotes.
- Legacy row with a 5,000-character or multi-line label → the error renders `user <uuid>`;
  `get_job`, `list_leases`, and `read_messages` return it as `null` (Addendum item 2).
- Error ordering: the label is checked in `of-core`, so anything the handler or `of-core`
  refuses first wins — an empty job list (`invalid_argument`), a job the caller cannot see
  (`job_not_found`, from `ensure_jobs_visible`), an unresolvable repo (`repo_unresolved`, from
  `repo_for_write`), or an empty / over-long lease `resource`. Every one of those is a refusal
  with nothing written, so the order is immaterial to safety; a caller fixing the first error
  may then meet `invalid_agent_label`.

## Assumptions

- **128 characters is the right limit.** No telemetry exists on real label lengths; the number
  is chosen from the documented example and realistic composite labels, with 2× headroom.
- **Trimming is not a "silent rewrite" in the sense CLAUDE.md forbids.** It never changes what a
  reader sees, and it already happens to lease `resource` and message `body`.
- **Blank ≡ absent** for `agent`. This is a (harmless) behavior change for both tools, not an
  extension of an existing rule: `acquire_lease`'s handler already folds a blank `resource` or
  `branch` to absent, but a blank `agent` has until now been stored as `""` by both tools. It is
  the same reasoning applied to the one remaining field, and a stored `""` rendered as
  `claimed by ` was never useful to anyone.
- **Breaking.** Refusing previously accepted input is a contract change even if no known client
  sends such input; erring on the side of the marker costs a minor-version bump.
- **The user-id fallback for a non-conforming legacy label is acceptable** rather than a
  migration (see Scope, Out).

## Risks & Open Questions

- A deployment with an agent that genuinely uses labels over 128 characters will see
  `invalid_agent_label` on its next claim. The error says exactly how to fix it.
- The deny-list may miss an exotic invisible code point. Its job is to stop the common tricks
  (newlines, bidi overrides, zero-width joiners, tag characters); the quoting in §3 is the
  backstop for anything it misses.
- Labels that pass the rule can still contain `@name` or markdown, which the tracker renders in
  the claim comment `of-trackers` posts. That is the same property as the `result`, `error`, and
  cancellation-note text posted on other transitions, and should be handled for all of them
  together if at all.
- Repo slugs in `RepoUnresolved` remain unbounded peer-chosen text in error prose until the
  follow-up issue (savvagent/otto-factory#206) lands.
