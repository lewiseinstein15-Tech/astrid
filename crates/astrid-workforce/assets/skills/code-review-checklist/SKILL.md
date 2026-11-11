---
name: code-review-checklist
description: A fixed review order — correctness, safety, tests, style — so reviews catch what matters before taste.
when_to_use: Use when reviewing a diff or PR, when asked to double-check changes, or before merging anything to a default branch.
---

# Code review checklist

Review in this order; stop wasting comments on style while correctness is still open:

1. **Intent**: does the diff do what the description claims? Read the test (or absence of one) first — it states the claimed behavior.
2. **Correctness**: trace the main path and one boundary case by hand. Off-by-one, reversed conditions, error paths that return success.
3. **Error handling**: every `unwrap`/`expect` is justified or replaced; failures degrade loudly, not silently.
4. **Security**: untrusted input at every boundary; secrets never logged or hardcoded; path traversal and injection surfaces checked.
5. **Concurrency/state**: shared mutable state; lock scope; anything that assumes single-threaded execution.
6. **Tests**: new behavior pinned by tests; the bug this change fixes would be caught by a regression test.
7. **Consistency**: naming and patterns match the surrounding code; no new dependencies without justification.
8. **Style last**: formatting belongs to tools, not humans.

Phrase findings as questions or requests with a reason ("this drops the error — can it log and continue instead?"), and say plainly which items you checked and found clean.
