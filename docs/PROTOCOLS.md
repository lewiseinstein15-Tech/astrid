# Astrid frozen protocols (v1)

Small contracts, versioned, fully wired. A protocol change bumps the version
and updates every caller in the same commit — there is no compatibility shim
layer because there is only one live path.

## 1. Memory protocol v1.0 — five verbs

Frozen in `astrid-memory::protocol`. Every memory mutation is a verb call;
tools and the agent never touch memory files directly.

| Verb | Signature | Effect |
|---|---|---|
| `remember` | `remember(topic, body, kind) -> PageId` | Writes/updates a markdown page under `notes/`, records provenance (turn, session, timestamp) |
| `recall` | `recall(query, k) -> Vec<RecallHit>` | BM25-lite + recency + phrase boost → RRF fusion; each hit carries per-stage scores |
| `entity` | `entity(name) -> Option<EntityPage>` | Typed entity page (id, kind, aliases, edges) from `entities/` |
| `synthesize` | `synthesize(topic, source_ids, body) -> PageId` | Writes a summary page under `summaries/` linking its sources |
| `forget` | `forget(id, reason) -> Tombstone` | Tombstones the page (frontmatter `forgotten: true` + reason); content never deleted |

Version: `PROTOCOL_VERSION = "1.0"`; envelope errors: `E_UNKNOWN_VERB`,
`E_BAD_ARG`, `E_NOT_FOUND`, `E_CONFINEMENT`.

## 2. Receipt / event taxonomy v1 — 14 events

Frozen in `astrid-core::events`. Every event is a serde enum variant persisted
as one JSON line in `.astrid/events.jsonl`.

| Event | Carries |
|---|---|
| `session.started` / `session.ended` | session id, config fingerprint |
| `turn.started` | turn id, input preview |
| `message.delta` | turn id, streamed text chunk |
| `thinking.delta` | turn id, reasoning chunk |
| `narration.line` | turn id, kind (`ack/finding/progress/decision/completion`), text |
| `tool.started` | call id, tool, redacted args, risk |
| `tool.completed` | call id, result preview, duration_ms |
| `tool.failed` | call id, error, duration_ms |
| `tool.refused` | call id, tool, reason (gate, approval, confinement) |
| `approval.requested` / `approval.resolved` | approval id, tool, finalized details / decision |
| `hook.fired` | hook event, script, decision |
| `turn.completed` | turn id, outcome (`completed`, `completed_fast_path`, `awaiting_approval`, `cancelled`, `failed`, `unverified`), provider, ladder_slid |

Redaction rule: receipt args truncate strings to 200 chars and blank any key
whose name matches `(?i)(key|token|secret|password|authorization)`.

## 3. Hook contract v1 — 10 wired events

`SessionStart, SessionEnd, UserPromptSubmit, PreToolUse, PermissionRequest,
PermissionDenied, PostToolUse, PostToolUseFailure, TurnCompleted, PreCompact`

- Registration: `.astrid/hooks.toml` → `{event, command, matcher?, timeout_ms?}`.
- Invocation: JSON payload on stdin; environment carries `ASTRID_EVENT`,
  `ASTRID_SESSION_ID`.
- Answer: exit `0` = allow; exit `2` = block (stdout is the reason); any other
  exit = warn (logged, never blocks).
- Fail-open on spawn error, and the fail-open itself emits a `hook.fired`
  event with `decision: "fail-open"`.
- Matcher: glob prefix on tool name (`fs_*`, `shell_run`, `*`).

## 4. Risk ladder & permission profiles v1

- Classes: `Safe` (read-only) → `Medium` (writes Astrid-owned data) →
  `High` (code execution) → `External` (network egress).
- Profiles: `readonly` (Safe only), `auto` (default: Safe/Medium run, High and
  External ask), `full` (everything runs; developer profile, warned at startup).
- Path confinement: every filesystem tool resolves through
  `confinement::resolve_within(root, path)` — canonicalized, segment-boundary
  checked, no `..` escape, symlink-resolved.

## 5. Provider ladder contract v1

- Order = config order. A rung serves only if configured (endpoint + model;
  key optional for keyless lanes). Rung failure ×3 → 30 s cooldown;
  Retry-After respected.
- `ladder_slid` (number of rungs skipped before the first serving rung) is
  reported on every turn — a silent slide is a measurement bug, not a feature.
- The offline provider is always last and always labeled `provider: offline`.
