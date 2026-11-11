# M5 — Agent OS: streaming, narration, themes, MCP, plugins

M5 turns Astrid from "an agent in a TUI" into an **Agent OS**: one registry
where builtins, MCP servers, and manifest plugins live side by side behind the
same receipt pipeline; a provider layer with real token streaming; a narration
engine that speaks through every phase of a turn; and a themed surface where
every role owns a hue.

## 1. Streaming (end to end)

The provider contract grew a streaming lane:

* `Provider::chat_stream(&req) -> Result<Receiver<StreamItem>>` — a channel of
  `Delta(String)` / `Thinking(String)` chunks closed by exactly one terminal
  item (`Final(ChatResponse)` or `Failed(String)`).
* `OpenAICompatProvider` implements real SSE (`"stream": true`): content
  deltas flow live, `reasoning_content` becomes `Thinking`, and tool-call
  fragments are re-assembled by index into the final `ChatResponse`.
* `ProviderLadder::chat_stream` walks rungs with the same skip/cooldown rules
  as `chat`, but commits to a rung only after its stream proves itself — a
  pre-commit `Failed` slides to the next rung exactly like an HTTP error.
* Providers without SSE (the offline planner included) fall back to the
  default: one `Final`, no deltas. The session fills that gap with **paced
  reveal** — word-boundary chunks at ~14 ms — so the transcript always streams.
* The TUI accumulates `MessageDelta` into a live streaming line (newline-aware),
  shows reasoning as a dim `◌ thinking…` indicator, and the status bar flips to
  a spinning `STREAMING` state while deltas flow.

## 2. Narration engine

A full agent says what it is doing. The turn loop now emits narration at every
phase, each kind with its own icon and hue in the transcript:

| Kind | Icon | When |
|---|---|---|
| `Ack` | `✦` | task accepted, step budget announced |
| `Decision` | `➤` | acting: tool + args; ladder slides; failures |
| `Progress` | `»` | step k/N; cancel requested |
| `Finding` | `◈` | slash-command answers |
| `Completion` | `✓` | answered after N model calls, M tool calls |

## 3. MCP (Model Context Protocol)

New crate `astrid-mcp`: a stdio JSON-RPC 2.0 client that spawns each
configured server, performs the `initialize` → `notifications/initialized`
handshake, discovers tools via `tools/list`, and exposes every tool as an
`McpToolAdapter` implementing the standard `Tool` trait. That means MCP calls
flow through the **same eight-stage pipeline** as builtins: unknown check →
started receipt (redacted args) → RiskGuard → PreToolUse hooks → permission
gate → approval → execute → terminal receipt.

Security posture (per the MCP spec's "tool annotations are untrusted"):
MCP tools default to **High** risk — the `auto` profile asks for approval
before the first external call. A trusted internal server can be downgraded
deliberately via `default_risk` in its config block. Server logs go to
`/dev/null` (never the TUI); a dead server fails honestly, never silently.

```toml
[mcp]
enabled = true
timeout_ms = 15000

[[mcp.servers]]
name = "demo"
command = "python3"
args = ["examples/demo-mcp-server.py"]
# default_risk = "medium"   # optional deliberate downgrade
```

`/mcp` lists each server's handshake result and tool count; `astrid doctor`
performs a real handshake against every configured server.

## 4. Plugins

Manifest-based external tools in `<data_dir>/plugins/<name>/plugin.toml`:

```toml
name = "wordcount"
description = "Count words in a piece of text"
risk = "safe"                      # default: high (a plugin is code execution)
run = ["bash", "{dir}/run.sh"]     # args JSON appended as one argv entry

[args_schema]
type = "object"
properties.text = { type = "string" }
required = ["text"]
```

No shell interpolation of model content: the manifest's argv template runs
with the tool arguments appended as a single JSON string, 15 s timeout,
kill-on-drop, output captured. Broken manifests are reported on `/plugins`
and `doctor` — a bad plugin is visible, never silent. The offline planner
reaches MCP and plugin tools through explicit patterns (`what time is it?`,
`generate a uuid`, `call the tool <name> with {json}`) that only ever target
tools actually offered — the menu never grows.

## 5. The surface: three palettes, one hue per role

`astrid-tui/src/theme.rs` ships `midnight` (GitHub-dark, default), `nordic`
(Nord), and `ember` (Gruvbox-inspired) truecolor palettes. Color carries
meaning, not decoration:

| Role | Hue (midnight) |
|---|---|
| user input | green `#7ee787` |
| assistant / streaming | off-white italic `#e6edf3` |
| narration ack / progress / decision / completion | blue / violet / orange / green |
| fs receipts | blue · shell orange · memory violet · skills teal · web cyan |
| mcp | gold `#f2cc60` · plugins pink `#ff9bce` |
| approval | magenta `#d2a8ff` · refusal amber · failure red |

Layout gained a header bar (agent, theme chip, provider, spinner), a rail with
grouped sections (session / providers with rung states / capabilities / tools
by family / MCP servers / plugins / approval panel), and a status bar
(`READY` / `WORKING` / `STREAMING` / `APPROVAL` chips plus lifetime counters).

`/theme <name>` switches palettes live; `/clear` wipes the transcript view
(receipts stay in `events.jsonl`).

Two long-standing defects were fixed as part of the rebuild: the transcript
now follows the tail (PageUp detaches, PageDown/End re-follows), and tracing
logs no longer corrupt the alternate screen (the subscriber initializes only
for headless modes).

## 6. Verification

* 142 tests across the workspace, including MCP integration tests against a
  real stdio server child (handshake, list, call, error, timeout), plugin
  scan/execute tests, SSE streaming tests against a hand-rolled chunked mock
  (deltas + tool-call re-assembly), and ladder streaming failover tests.
* Live TUI demo (`scripts/astrid-demo/driver2.py`): 12 turns — small talk,
  capability answer, fs write, memory remember/recall, approved shell run,
  RiskGuard refusal, two approved MCP tool calls, a plugin execution,
  `/mcp` / `/plugins`, live `/theme` switches — with receipts pairing exactly
  (8 started = 7 completed + 1 refused; 3 approvals requested and resolved).
