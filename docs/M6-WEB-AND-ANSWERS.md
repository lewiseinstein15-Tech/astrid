# M6 — Web search + clean answer rendering

Two upgrades to the "answer" experience: Astrid can now **search the live
web** (in addition to fetching URLs), and every finished answer renders as a
**clean document-style block** instead of a raw tool dump.

## 1. `web_search` — the 15th builtin

A real search tool, approval-gated (risk `External`, so the `auto` profile
asks before the first call):

* Backend 1: DuckDuckGo Instant Answers (keyless JSON: abstract + related
  topics). Uneven coverage — an empty response means *no results*, not
  failure, and the ladder moves on.
* Backend 2: the Wikipedia search API (MediaWiki `list=search`), with the
  descriptive User-Agent Wikimedia requires. Structured JSON: title, snippet
  (HTML stripped), `curid` permalink per hit.
* Both backends failing is an honest error naming both; a successful search
  with zero hits is also an honest error.
* Output is markdown from the start — `- **Title** — snippet` bullets with the
  link on its own line — so the composer and the renderer never have to parse
  fragile prose.

The offline planner routes to it deterministically:

```
search the web for rust programming language
look the web up for <query>
fetch https://example.com [and tell me what it is]
```

and only when `web_search` / `web_fetch` are actually offered — the menu
never grows by pattern-matching.

## 2. The answer composer

After a tool observation, the offline agent composes the conclusion **per
tool** — arrangement only, never invention. Every fact comes from the tool's
own output:

| Tool | Answer shape |
|---|---|
| `web_search` | intro + the tool's markdown bullets with links |
| `web_fetch` | "here's the gist" — word-wrapped excerpt + `Source:` line |
| `shell_run` | "Ran \`cmd\` — exit 0." + indented stdout block |
| `fs_write` | confirmation + target path |
| `fs_read` | contents as a fenced code block |
| `mem_remember` / `mem_recall` | structured note / hit list |
| anything else | tool name + capped output (the generic fallback) |

The old `Done. Tool result: …` dump is gone; `cargo test` locks the new
shapes in.

## 3. The clean answer block (TUI)

While streaming, the transcript keeps the raw typewriter effect. When the
turn completes, `flush_stream` **replaces** the raw typing lines with a
rendered block built from the full answer text:

* an agent header rule (`▌ astrid` in the accent hue),
* `#` headings as bold accent rules,
* `- `/`* ` bullets as colored `• ` markers,
* `>` quotes as `❝` lines, fenced code as indented `│` lines,
* inline `**bold**`, `` `code` `` and bare URLs styled by a one-pass
  tokenizer (`inline_spans`) — markers are consumed, never printed,
* a blank rule after the block.

Fast-path answers (small talk, capability questions) get the same block, so
every response in the transcript reads like a finished document. The
streaming line, thinking indicator and tail-following behavior are unchanged.

## 4. Verification

* `web_search` tests: DDG formatting, DDG→Wikipedia fallback (with snippet
  HTML stripped), both-backends-fail honest error, missing-arg refusal.
* Composer tests: search answers keep bullets + links; shell answers carry
  command, exit code and stdout — with stderr scaffolding removed.
* TUI tests: flush replaces raw lines exactly once (no duplicated text),
  bullets/headings/code/quotes render, inline markers are consumed, bold
  spans are actually styled, fast-path answers get the block too.
* Live demo (`scripts/astrid-demo/driver3.py`): a session where the user asks
  for a web search, approves the gate, and the real results type out into a
  finished clean block — recorded frame by frame.
