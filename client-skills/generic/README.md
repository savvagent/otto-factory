# Generic MCP client — session-start marker (contribution wanted)

**Target client:** any MCP client without a dedicated template of its own (`id: 'generic'`
in the console's `web/src/lib/clients.ts` — "exists so that a client nobody here has heard
of is still a first-class citizen: it is the endpoint and the two discovery documents,
which is all any conforming MCP client actually needs").

**Status: stub.** No working hook code ships here yet, and — unlike the other stubs in this
directory — none is likely to, since `generic` names no single client with its own
automation surface to target. See `../README.md`'s "Launch scope" note for why this
directory ships honest stubs rather than fabricated, unverified templates.

**What a contribution here would actually look like:** this entry exists so the directory
layout matches `web/src/lib/clients.ts::CLIENTS`'s `id` values exactly, not because a
generic session-start mechanism is expected to appear. If you maintain an MCP client that
otherwise has no dedicated template in this directory and it exposes some equivalent of a
session-start lifecycle hook, the right move is to propose a **named** template for that
client (following `../claude-code/README.md`'s worked example or `../README.md`'s
contract directly) rather than a template here — `generic` in the console refers to "any
client," not to one specific automation surface this directory could target.

## Contributing this template

Follow `../README.md`'s contribution checklist exactly, in a subdirectory named for your
actual client rather than this one, if what you have in mind is a specific client's
session-start automation.
