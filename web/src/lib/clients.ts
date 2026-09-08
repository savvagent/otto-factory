/**
 * How to point a coding agent at this server.
 *
 * A data table rather than a page full of branches, and every entry has the
 * same shape, because **no client is first-class here**. otto-factory is
 * coding-agent agnostic by constraint: Claude Code, Copilot CLI, Cursor, Codex
 * and anything else speaking MCP are equally supported, nothing depends on one
 * client's plugin or hook system, and `agentType` is never validated against a
 * list. A console that gave one of them a bespoke wizard and the rest a
 * footnote would be the first place that promise quietly broke.
 *
 * Every client gets two forms:
 *
 * - **OAuth** — the intended path. The client discovers the authorization
 *   server from `/.well-known/oauth-protected-resource`, registers itself
 *   (RFC 7591), and sends the human here to consent. No secret is ever pasted.
 * - **Token** — the compatibility path, for a client whose OAuth support is
 *   partial. A personal access token lands in the same table as an OAuth access
 *   token with the same audience and the same scopes, so nothing downstream can
 *   tell which it received.
 *
 * `generic` exists so that a client nobody here has heard of is still a
 * first-class citizen: it is the endpoint and the two discovery documents, which
 * is all any conforming MCP client actually needs.
 *
 * ## What is translated here, and what is not
 *
 * **Prose is translated; anything a machine reads is verbatim.** `note` is
 * advice to a person. `label` is prose for exactly one entry — "Any other MCP
 * client" is a description, where `Claude Code` and `Cursor` are product names
 * and stay as they are in every language. Commands, JSON and TOML snippets, and
 * `location` paths like `~/.copilot/mcp-config.json` are never touched: a
 * translated `--transport http` is a broken command and a translated config
 * path is a file nobody has.
 *
 * `label` and `note` are functions rather than strings because this module is
 * evaluated at import time, which can precede `resolveAtBoot()`. Resolving a
 * message eagerly here would freeze the base locale into the table.
 */

import { m } from '$lib/paraglide/messages';

export interface ClientRecipe {
  id: string;
  /** What to call it in the picker. A product name, or — for `generic` — prose. */
  label: () => string;
  /** How the snippet should be syntax-labelled, and what the reader is meant to do with it. */
  kind: 'command' | 'json' | 'toml';
  /** Where a config-file snippet belongs, when it is a file rather than a command. */
  location?: string;
  oauth: (mcpUrl: string) => string;
  token: (mcpUrl: string, token: string) => string;
  note?: () => string;
}

const PLACEHOLDER = 'of_pat_…';

export const CLIENTS: ClientRecipe[] = [
  {
    id: 'claude-code',
    label: () => 'Claude Code',
    kind: 'command',
    oauth: (url) => `claude mcp add --transport http otto-factory ${url}`,
    token: (url, token) =>
      `claude mcp add --transport http otto-factory ${url} \\\n  --header "Authorization: Bearer ${token || PLACEHOLDER}"`,
    note: () => m.client_note_claude_code()
  },
  {
    id: 'copilot-cli',
    label: () => 'Copilot CLI',
    kind: 'json',
    location: '~/.copilot/mcp-config.json',
    oauth: (url) =>
      JSON.stringify({ mcpServers: { 'otto-factory': { type: 'http', url } } }, null, 2),
    token: (url, token) =>
      JSON.stringify(
        {
          mcpServers: {
            'otto-factory': {
              type: 'http',
              url,
              headers: { Authorization: `Bearer ${token || PLACEHOLDER}` }
            }
          }
        },
        null,
        2
      ),
    note: () => m.client_note_copilot()
  },
  {
    id: 'cursor',
    label: () => 'Cursor',
    kind: 'json',
    location: '~/.cursor/mcp.json, or .cursor/mcp.json in a project',
    oauth: (url) => JSON.stringify({ mcpServers: { 'otto-factory': { url } } }, null, 2),
    token: (url, token) =>
      JSON.stringify(
        {
          mcpServers: {
            'otto-factory': {
              url,
              headers: { Authorization: `Bearer ${token || PLACEHOLDER}` }
            }
          }
        },
        null,
        2
      )
  },
  {
    id: 'codex',
    label: () => 'Codex CLI',
    kind: 'toml',
    location: '~/.codex/config.toml',
    oauth: (url) => `[mcp_servers.otto_factory]\nurl = "${url}"`,
    token: (url, token) =>
      `[mcp_servers.otto_factory]\nurl = "${url}"\n\n[mcp_servers.otto_factory.http_headers]\nAuthorization = "Bearer ${token || PLACEHOLDER}"`
  },
  {
    id: 'generic',
    label: () => m.client_generic_name(),
    kind: 'command',
    oauth: (url) => url,
    token: (url, token) => `Authorization: Bearer ${token || PLACEHOLDER}\n\n${url}`,
    note: () => m.client_note_generic()
  }
];
