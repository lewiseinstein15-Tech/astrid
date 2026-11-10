# Astrid Architecture

This document is the load-bearing design record. Every crate, every protocol,
and every decision boundary is listed here. The rendered diagrams live in
`docs/assets/architecture.svg` and `docs/assets/infrastructure.svg`.

## 1. Shape

Astrid is a **single Rust binary** composed of a cargo workspace of seven
crates. There is no server, no socket, no browser. The TUI is the only surface;
the same binary also exposes three headless modes (`ask`, `eval`, `doctor`)
that drive the identical runtime for scripting and CI.

Dependency graph (arrows point at what is depended on):

```
                 ┌──────────────┐
                 │ astrid (bin) │  composition root, turn loop, eval, doctor
                 └──────┬───────┘
        ┌──────────┬────┴─────┬───────────┐
        ▼          ▼          ▼           ▼
  ┌──────────┐ ┌────────┐ ┌─────────┐ ┌──────────┐
  │astrid-tui│ │astrid- │ │astrid-  │ │astrid-   │
  │          │ │tools   │ │providers│ │workforce │
  └────┬─────┘ └───┬────┘ └────┬────┘ └────┬─────┘
       │           │  └────────┼───────────┤
       │           ▼           │           │
       │      ┌──────────┐     │           │
       │      │astrid-   │◄────┘           │
       │      │memory    │                 │
       │      └────┬─────┘                 │
       ▼           ▼                       ▼
       └────────► astrid-core ◄────────────┘
        events · receipts · config · kernel · hooks
```

Rules that keep this graph honest:

- **`astrid-core` depends on nothing internal.** It defines the vocabulary
  every other crate speaks: `Event`, `Receipt`, `Risk`, `AstridConfig`,
  `HookEvent`, the intent gate, and the error type.
- **No crate may depend on `astrid` (the bin).** The bin is the only place that
  wires concrete implementations together. This makes every crate testable in
  isolation and prevents the "second generation stack" drift Astrid was rebuilt
  to escape.
- **One live path per concern.** If two crates could do the same job, one is
  deleted, not deprecated. The port map (docs/PORT-MAP.md) records the cuts.

## 2. A turn, end to end

1. The TUI (or headless mode) sends the user text to the **session** (bin crate).
2. The **kernel gate** (`astrid-core::kernel`) classifies the input:
   - `SmallTalk` → canned reply, zero model calls, turn closes honestly
     (`outcome: completed_fast_path`).
   - `CapabilityQuestion` ("what tools do you have?") → answered from the live
     tool registry and provider state, zero model calls.
   - `Task` → the agent turn loop.
3. The **turn loop** (bin crate, bounded at `max_steps = 8`):
   - builds the system prompt (constitution + tool menu + memory recall excerpt
     + instincts), respecting the prompt budget;
   - calls the **provider ladder** (`astrid-providers`): first configured
     provider that is past cooldown wins; a rung counts as a model call only if
     that provider is actually configured — a slid ladder is reported;
   - when the model returns tool calls, each call goes through the
     **tool pipeline** (below); results are observed back into the loop;
   - malformed or missing tool calls get one bounded recovery attempt
     (alias bridge + fenced-code extraction) before the turn ends;
   - the turn ends with an honest outcome: `completed`, `completed_fast_path`,
     `awaiting_approval`, `cancelled`, `failed(reason)`, or `unverified`.
4. Every step publishes `Event`s to the **event bus** (`tokio broadcast`).
   The TUI subscribes and renders; the session also appends every event to
   `.astrid/events.jsonl` (append-only, one JSON object per line).

## 3. Tool pipeline (every call, every time)

```
tool call args
   │
   ▼
[1] plan gate ........... execution tools refused while a plan is pending
[2] PreToolUse hook ..... external scripts, exit-code contract, fail-open
[3] permission gate ..... deny-by-default grant + risk ceiling per profile
[4] RiskGuard ........... classifies the ACTUAL args (rm -rf, curl|sh, …)
[5] ring guard .......... sandbox < container < host; high-risk needs ring ≥ level
[6] approval ............ high/external tools pause the turn; TUI renders an
                          approval row; deny → receipt `refused`
[7] execute ............. the real tool, under path confinement
[8] receipt ............. tool.started → tool.completed / tool.failed /
                          tool.refused, with redacted args + duration
```

Risk ladder: `Safe` (read-only) → `Medium` (writes Astrid's own data) →
`High` (code execution) → `External` (network egress). Profiles:
`readonly`, `auto` (default; asks for high/external), `full` (dev only).

## 4. Memory (astrid-memory)

Markdown-first, rebuildable-from-repo, the part of Jexi's brain that worked:

- Pages are markdown files under `.astrid/memory/<kind>/`, synced against a
  sha256 manifest. No database. `forget` tombstones; history is never deleted.
- The **five-verb protocol v1** (`recall, remember, entity, synthesize, forget`)
  is frozen in `protocol.rs`; all writes go through the verbs, never raw files.
- Retrieval is **BM25-lite + recency + exact-phrase boost**, merged with a
  deterministic RRF fusion, with per-stage scores attached to results — the
  honest, dependency-free subset of Jexi's hybrid search.

## 5. Workforce (astrid-workforce)

- The roster is **curated, not generated**: 12 agents across 5 divisions, each
  declaring its capabilities explicitly in TOML. No keyword-inference magic.
- `SKILL.md` discovery walks project `.astrid/skills/` then the bundled
  `skills/` directory, parses YAML-frontmatter (a small hand-rolled parser —
  the used subset only), ranks, and exposes progressive disclosure
  (`skills_list` gives metadata; `skills_load` fetches one body).

## 6. Hooks (astrid-core::hooks)

Astrid ships exactly the **10 hook events it fires**:
`SessionStart, SessionEnd, UserPromptSubmit, PreToolUse, PermissionRequest,
PermissionDenied, PostToolUse, PostToolUseFailure, TurnCompleted, PreCompact`.
External scripts receive JSON on stdin and answer by exit code:
`0` = allow, `2` = block (reason on stdout), other non-zero = warn.
Spawn errors **fail open** (documented), because a broken hook script must not
brick the runtime — that choice is printed in the receipt trail.

## 7. Providers (astrid-providers)

- One trait, `Provider`: `chat(ChatRequest) -> ChatResponse` with native
  OpenAI-style tool calling.
- The **ladder** walks configured providers in order, with a 30 s cooldown
  after 3 consecutive failures and Retry-After awareness. Locality is decided
  by endpoint (loopback = local), so `local_only = true` excludes keyless
  remote lanes — this is the hermetic switch.
- The **offline agent** is a first-class provider: deterministic, template- and
  registry-driven answers, explicitly labeled in every receipt trail as
  `provider: offline`. Astrid never pretends a fallback was a model.

## 8. Storage layout

```
.astrid/
├── config.toml          # providers, permissions profile, hooks, ui
├── events.jsonl         # append-only receipt/event log
├── sessions/<id>.json   # session state (resumable)
├── memory/
│   ├── manifest.json    # path → sha256 sync ledger
│   ├── notes/           # remember-verb pages
│   ├── entities/        # entity-verb pages
│   └── summaries/       # synthesize-verb pages
├── skills/              # project skill overrides (SKILL.md)
└── hooks.toml           # external hook registrations
```

## 9. Testing posture

- Every crate carries unit tests; `cargo test --workspace` is hermetic: the
  provider client never dials unless a test explicitly binds a local server
  (`local_only` enforced in tests).
- `astrid eval` runs the real turn loop against a task file and grades the
  real artifacts; the report records provider, ladder slides, and per-task
  receipts. A skipped check is `unverified`, never `pass`.
- The eval harness is the honesty instrument: numbers come from runs, the run
  JSON is committed alongside any claim.
