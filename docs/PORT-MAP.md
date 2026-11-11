# Port map: Jexi OS → Astrid

Astrid is a clean-room rebuild in Rust. Nothing was translated line-by-line;
the Jexi tree was studied end-to-end (server runtime, brain, workforce, hooks,
security gates, benchmarks, and its own AGENTS.md engineering log) and each
piece was either kept by design, cut, or redesigned. This document records
those decisions and why — it is the "clean up dead code" contract: Astrid
ships no module that the binary does not load.

## Kept (redesigned for Rust)

| Jexi concept | Astrid form | What changed |
|---|---|---|
| Event stream + tool receipts (`tool.started/completed/failed`) | 14-event frozen taxonomy in `astrid-core::events`, append-only `events.jsonl` | One wire format everywhere (Jexi had two: server `tool.start/result` vs console `tool.started/completed`) |
| Five-verb memory protocol (recall/remember/entity/synthesize/forget) | `astrid-memory` protocol v1, same verbs, frozen | Markdown store with sha256 manifest; two Jexi memory systems (Phase-17 JSONL + Phase-28 brain) merged into one |
| Permission gates: plan gate → PreToolUse hook → permission gate → RiskGuard → ring guard → approval | Same six-step pipeline in `astrid-tools` | Risk classes named for what they protect (safe/medium/high/external); terminal-input classified like code execution, closing the `terminal_send` bypass class |
| Provider ladder with cooldowns, keyless floor, `local_only` hermetic switch | `astrid-providers` | Locality decided by endpoint, not key absence (Jexi's NetworkPolicy lesson, kept) |
| Kernel intent gate: small talk / capability questions answered with zero model calls | `astrid-core::kernel` | Deterministic regex gate; capability answers come from the live registry only |
| Bounded tool-call recovery (alias bridge, fenced-code extraction) | turn loop recovery pass | One bounded attempt, honestly reported in the receipt trail |
| keyRef discipline (keys referenced by env-var name, never inline) | `key_ref` in config, enforced in provider setup | No UI copy-paste path exists at all |
| Hooks: external scripts, JSON stdin, exit-code contract | `astrid-core::hooks` | Catalog reduced to the 10 events actually fired |
| Honest eval harness (HumanEval-live lessons) | `astrid eval` | Per-task receipts, provider/ladder recorded, `unverified` ≠ `pass` |
| Skills catalog with progressive disclosure | `astrid-workforce` skills | ~8 bundled curated skills vs 1,164 vendored files |

## Cut (with reasons)

| Jexi piece | Why cut |
|---|---|
| Web console (Vite/React), HUD/SSE, Android/Capacitor APK, deck | Astrid is TUI-only by charter |
| 400-agent / 18-division workforce registry | In Jexi it was never loaded by the server — a cosmetic registry. Astrid ships 12 agents that all run |
| MCP (56 servers consumed + server exposure) | v0.1 scope boundary; real client planned for v0.2. Cut to keep the dependency tree honest |
| surfsense research engine, 20 search engines, 16 connectors | Standalone in Jexi (probe-scripts only). Astrid keeps `web_fetch` only |
| Swarm hive/topologies/consensus, omnia council, evomap, semantica | Unwired museum modules in Jexi; delegation is one `agent_delegate` tool with a restricted toolset |
| Computer use (two parallel stacks in Jexi) | Out of scope for a terminal OS |
| LSP tools, data (csv/sql) tools, git/github tools | Worth having, but v0.1 keeps the tool catalog at 14 fully-implemented tools; extensions follow the same pipeline |
| Docker/Render/Cloudflare/free-tier deploy matrix | One binary; `cargo build` is the install |
| Benchmark skeletons (GAIA/SWE-bench/OSWorld/WebArena/Terminal-bench) | Jexi shipped loaders never run; Astrid ships only the harness it runs (`eval`) |
| 30-event hook catalog (24 declared no-ops) | A contract should list what fires; Astrid's catalog = wired set |
| `.jexi/` declarative config skeleton | Config that exists must be read; Astrid's `config.toml` is live |

## Redesigned away (the failure modes Astrid exists to avoid)

1. **Duplicate generations.** Jexi carried two tool stacks, two memory systems,
   two instinct systems, three rosters, and four console surfaces. Astrid: one
   per concern, enforced by the crate graph (no crate can shadow another's job).
2. **Unwired registries.** Anything in Astrid's tree is imported by the bin.
   The eval harness greps for orphans as part of `astrid doctor`.
3. **Numbers without artifacts.** Jexi's discipline of committing run JSONs is
   kept, but the surface shrinks to one harness so no skeleton can drift.
4. **Config that isn't read.** Every key in `config.toml` is parsed into a
   typed struct; unknown keys are warnings at startup, not silence.
