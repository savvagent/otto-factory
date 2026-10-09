# Back-to-platform link design

> **Status:** DRAFT — a persistent "Back to otto" link in the console header, pointing at the
> otto platform's console, built from addresses the server reports at runtime. Closes
> savvagent/otto-factory#212.

## Addendum: review round 1 (PR #213)

Where the body below disagrees with this section, this section wins.

1. **Every platform link is scheme-checked** (security review, Low). The Manage menu built hrefs
   from the raw `platformUrl`. The layout now derives one `platformBase =
   platformHome(session.platformUrl)` and builds both the back link and the Manage menu from it.
   The menu is hidden when the base is `undefined`.
2. **`platformHome` refuses a URL that carries credentials, and returns origin plus path only**
   (security review, Low; code review). That way `user:pass@` can never reach every visitor's
   DOM, and a query or fragment is dropped instead of being slash-trimmed.
3. **Discovery runs whenever the session gives no usable address, not only when signed out**
   (Rust review). A known session address is also remembered in `discovered`, so signing out
   does not blank the link and needs no fetch (architect and code review).
4. **The coupling is pinned by a test** (architect review, Important).
   `the_console_and_the_discovery_document_name_the_same_platform` in
   `crates/of-server/src/lib.rs` asserts that `of-mcp`'s `platform_url` (which feeds
   `authorization_servers`) and `of-web`'s `platform_url` (which feeds `/api/session`) are the
   same value. A follow-up could move the address into a documented unauthenticated answer if
   the platform ever separates its authorization server from its console.
5. **"Every page" is narrower in practice.** On a gated page, a signed-out visitor is
   redirected to sign-in at once. So the signed-out link is really seen on `/`, `/docs/api`,
   and the error states (`fatal`, "sign-in stuck").

## Brief

> "otto-factory needs to provide a way in the UI to make it back to otto.savvagent.com"

A person signs in at the platform's console (otto.savvagent.com), then comes to
otto-factory.savvagent.com. The console gives them no obvious way back.

## Premise corrections

- The console already links into the platform: the header's **Manage** menu
  (`web/src/routes/+layout.svelte`) lists Members, Teams, SSO, Usage, Account settings, and
  "Switch organization", the last of which is the platform's root
  (`platformLink(_, 'orgs', _)`). It is not *obvious* — it is a closed `<details>` menu labelled
  "Manage" — and it renders only while signed in.
- The platform's address is not only in `GET /api/session`. The open, unauthenticated
  `GET /.well-known/oauth-protected-resource` (`crates/of-mcp/src/auth.rs`,
  `protected_resource_metadata`) names the platform in `authorization_servers`, built from the
  same `OF_PLATFORM_URL` (`crates/of-server/src/lib.rs`, `mcp_config`) that `/api/session`'s
  `platformUrl` comes from (`web_config`). So a signed-out visitor's console can still learn the
  platform's address at runtime, with no hard-coded fallback.

## Goal & success criteria

A clear, persistent way back to the platform's console from every console page.

- Signed in: a visible "Back to otto" link in the header, to the platform's root, from
  `session.platformUrl`.
- Signed out, or `/api/session` failed: the same link, from
  `/.well-known/oauth-protected-resource`'s first `authorization_servers` entry.
- Neither address available, or not an absolute `http(s)` URL: no link. Never a guessed URL.
- New strings exist in all six locales; `npm run check`, `lint`, `test`, `build` pass.

## Scope

**In:** `web/` only — a header link, a helper in `web/src/lib/platform.ts`, catalog entries,
unit tests.

**Out:** any server change (both addresses are already served); changing the Manage menu (it
stays, for the deep links); a platform-side link back to otto-factory (another repository).
The three constraints are untouched: no repo, workflow, or client-specific surface is involved.

## §1 Where the link sits and what it says

The header's right-hand group, first item, before the user's email and the sign-out/sign-in
button — present on every page, signed in or out, including the `fatal` and "sign-in stuck"
states, because the header renders outside the main-content gate.

Visible text: `← otto` is too terse for a screen reader, so the link's visible text is
"Back to otto" (`nav_back_to_platform`), with the arrow as an `aria-hidden` glyph. "otto" is a
product name and stays verbatim in every locale. It is the only new catalog key. The link's
`title` is the destination's bare host (not a translated sentence), so a hover shows where it
goes. Styling matches the existing header controls (`rounded-md`, `text-muted`,
`hover:bg-raised`); the focus ring is the global `:focus-visible` rule in `app.css`. Same tab, no
`target="_blank"`: going back is navigation, not a side trip.

Below the `sm` breakpoint the full label overflows a signed-in header at 375px, so the visible
text shrinks to "otto" (the product name, identical in every locale) while
`aria-label="Back to otto"` (translated) keeps the accessible name the same at every width.

## §2 Where the address comes from

`platformHome(url: string | undefined): string | undefined` in `platform.ts`: parses the value;
returns the origin-plus-path with trailing slashes stripped when it is an absolute `http:` or
`https:` URL, otherwise `undefined`. The scheme check is defence in depth — both sources are
our own server — so a misconfigured or corrupted value can never become a `javascript:` href.

`discoverPlatformUrl(fetcher = fetch): Promise<string | undefined>` fetches
`/.well-known/oauth-protected-resource` (same origin, no credentials needed), reads
`authorization_servers[0]`, and returns it through `platformHome`. Any failure — network,
non-2xx, bad JSON, missing field — returns `undefined`. Silence is right here, and only here:
the link is a convenience, and its absence is exactly the specified fallback.

The layout derives `home = platformHome(session.platformUrl) ?? discovered`. Discovery runs
once, whenever the session has resolved without a usable address — signed out, failed, or an
address `platformHome` refused (addendum item 3): a signed-in session already carries the
address, so the extra request is not made. That decision lives in `PlatformHome`
(`web/src/lib/platform-home.svelte.ts`), where it is unit-tested.

## Error handling & edge cases

- `/.well-known` unreachable or malformed → link hidden.
- Session later resolves signed in → the session's address wins.
- Platform URL with a path prefix (`https://x.example/otto/`) → kept, trailing slash dropped.

## Testing

`web/src/lib/platform.test.ts`: `platformHome` accepts http(s), strips trailing slashes, refuses
`javascript:`, relative, empty, and undefined; `discoverPlatformUrl` returns the first
authorization server, and `undefined` for non-2xx, bad JSON, a missing/empty array, a non-http
URL, and a thrown fetch. Visual check of the header against `npm run dev` with Playwright,
stubbing `/api/session` and `/.well-known/oauth-protected-resource` (signed in; signed out;
session `500` with discovery up; both down, so no link; 375px width; keyboard focus).

## Assumptions

- "Back to otto" (not "otto.savvagent.com") is the right label: the host differs per deployment
  and the brand is the stable name. The `title` carries the host.
- The platform's root is the right destination (its own console decides where a signed-in
  person lands), matching the existing "Switch organization" link.
- The `.well-known` `authorization_servers` entry is the platform's console origin. Both values
  come from `OF_PLATFORM_URL`; if they ever diverge, signed-in visitors still get `platformUrl`.

## Risks & open questions

- If a future deployment splits the platform's authorization server from its console origin,
  the signed-out link would point at the authorization server. Acceptable: a signed-out visitor
  going there is still going "back to otto".
