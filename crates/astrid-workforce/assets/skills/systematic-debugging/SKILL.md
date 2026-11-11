---
name: systematic-debugging
description: A four-phase debugging discipline that finds root causes instead of patching symptoms.
when_to_use: Use when a bug resists the first obvious fix, when failures are intermittent, or before declaring any "probably fixed" change done.
---

# Systematic debugging

1. Reproduce deterministically. A bug you cannot trigger on demand cannot be verified fixed; reduce it to the smallest command, input, or seed that shows it.
2. Read the actual error before theorizing. The stack trace, log line, or failing assertion is evidence — a guess is not.
3. Form one hypothesis at a time and state, out loud, the experiment that would falsify it.
4. Change exactly one variable per experiment. Two simultaneous changes make causation unattributable.
5. Bisect when the search space is large: `git bisect` for history, comment-out halves for code, delete layers for systems.
6. Check boundaries first: empty input, zero, maximum values, unicode, clock skew, concurrency, and the code path nobody thought to test.
7. Fix the cause, not the symptom. A guard that silences the crash is not a fix unless you can explain why the guard is correct.
8. Prove the fix: rerun the reproducer, run the surrounding tests, then look for sibling bugs of the same class elsewhere.

Finish by writing the regression test that would have caught this, so the bug can never return silently. If you cannot explain the root cause in one sentence, you have not found it yet.
