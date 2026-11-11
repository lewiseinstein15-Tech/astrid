---
name: verification-before-completion
description: A completion gate that replaces "I think it works" with evidence from real runs.
when_to_use: Use before reporting any task done — especially refactors, config changes, and anything the request framed as "quick".
---

# Verification before completion

Never report a task as done on the strength of having written the change. A claim of completion carries evidence:

1. Re-read the original request and list its explicit requirements. Check each one against the actual artifact, not against your intention.
2. Build it: the real build command, full output, zero new warnings. A build that "should work" has not been built.
3. Run it: the relevant tests, plus the command a user would actually run. Show or record the result.
4. For UI or documents, look at the rendered result — generated-but-never-looked-at is the most common failure mode.
5. Search the artifact for leftovers: placeholders, TODOs, commented-out code, wrong names from copied templates.
6. Verify numbers by recomputation, not memory; re-run the script that produced them if needed.

Say exactly what you verified and how. Anything not verified is reported as such — "unverified" is an honest outcome, a false "done" is not.
