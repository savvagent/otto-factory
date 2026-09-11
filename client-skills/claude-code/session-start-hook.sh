#!/usr/bin/env bash
# session-start-hook.sh — otto-factory session-start marker for Claude Code.
#
# Fired by Claude Code's SessionStart hook (matcher: "startup" only — see
# settings-snippet.json; a `resume` or `clear` restart of an existing session
# is not a new work session and must not re-fire this).
#
# This script cannot call any otto-factory MCP tool itself: a SessionStart
# hook is a detached subprocess with no access to the session's own
# authenticated MCP connection. Instead it captures the three facts the
# otto-factory calls need (remote URL, branch, date) and hands the model an
# instruction to perform those calls itself, via Claude Code's
# `additionalContext` SessionStart mechanism. See ./README.md for the full
# design and known gaps.
set -euo pipefail

# Claude Code pipes a JSON payload on stdin that includes a `cwd` field
# naming the project directory. Read it defensively and `cd` there
# explicitly rather than assuming the hook process's own working directory
# is already the project root.
input="$(cat)"
project_dir="$(printf '%s' "$input" | sed -n 's/.*"cwd"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
if [ -n "$project_dir" ] && [ -d "$project_dir" ]; then
  cd "$project_dir"
fi

# No git repo, no `origin` remote, or a detached HEAD: there is nothing
# stable to key an idempotency key on. Exit 0 with no stdout — no
# additionalContext is ever injected, so the model never even sees an
# instruction to act on.
remote="$(git remote get-url origin 2>/dev/null)" || exit 0
branch="$(git rev-parse --abbrev-ref HEAD 2>/dev/null)" || exit 0
if [ -z "$remote" ] || [ "$branch" = "HEAD" ]; then
  exit 0
fi
today="$(date -u +%Y-%m-%d)"

# Minimal JSON-string escaping (backslash, double-quote, newline) so the
# instruction text below can be embedded as a JSON string value without
# assuming `jq` (or any other JSON tool) is installed on the developer's
# machine.
json_escape() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/\\n}"
  printf '%s' "$s"
}

instruction="$(cat <<INSTRUCTION_EOF
You are running inside a Claude Code session that just started (a SessionStart hook fired, matcher: startup). Silently and without narrating any of this to the developer — no preamble, no summary of what you are doing, whether it succeeds or fails — perform the following steps exactly, using your own already-authenticated otto-factory MCP tools:

1. Call resolve_repo with remote set to "${remote}". If this fails for any reason (the repo is not registered, there is no otto-factory MCP connection configured, or the call errors), stop here and do nothing further. Do not mention this to the developer.
2. On success, take the resolved repo's canonical slug from the response. Call add_job with:
   - repo: <that slug>
   - title: "session: ${branch}"
   - description: "Automatic session-start marker created by client-skills/claude-code's SessionStart hook. No work was performed under this job; it exists only to record that a development session began on this branch."
   - metadata: {"kind": "session-marker", "source": "client-skills/claude-code", "branch": "${branch}"}
   - idempotencyKey: "session-<slug>-${branch}-${today}" (use the exact same slug value you got from resolve_repo here and in the repo argument above — never derive the repo identifier a second, independent way)
3. Branch on the returned job's status:
   - completed: stop here. Today's marker for this branch already exists and is already closed out. Do nothing further.
   - pending: call claim_jobs with jobs set to a one-element array containing that exact job id. If the claim fails (for example, a concurrent duplicate hook invocation claimed it first), stop here.
   - anything else (in practice, in-progress): stop here without calling complete_job. Do not guess whether you are the current claim holder.
4. Only after a successful claim in step 3: call complete_job on that same job id with result set to "session marker — no work performed".

Whatever happens above, do not narrate it in your reply to the developer: no announcement of success, and no mention of any failure either. The tool calls themselves still appear in the normal tool-call transcript — this instruction only asks you to stay silent about them in your own prose reply. Never let this delay, block, or take priority over addressing whatever the developer actually asked in their first message.
INSTRUCTION_EOF
)"

instruction_escaped="$(json_escape "$instruction")"

printf '{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"%s"}}\n' "$instruction_escaped"
