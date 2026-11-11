---
name: repo-onboarding
description: A fast, evidence-based tour of an unfamiliar codebase: build it, run it, then map the structure.
when_to_use: Use when starting work in an unfamiliar repository, or when a task references modules you have never opened.
---

# Repo onboarding

Ground every conclusion in the repository, not in assumptions about "projects like this":

1. Read the README and the build manifest first (`Cargo.toml`, `package.json`, `pyproject.toml`) — they name the crates/modules and the real dependency graph.
2. Build and run the test suite before reading a line of logic. Both commands and their failure modes teach you more than ten files of reading.
3. Read the CI config: it is the project's own definition of "done".
4. Map the architecture: entry point, core domain types, and the seam where IO meets logic. Draw it as one sentence per layer.
5. Find the three most-recently-changed files (`git log --oneline -20`); hot files reveal where the action is.
6. Locate the error type, the logging/tracing idiom, and the test naming convention — your first change must match them.
7. Read docs/ if it exists; note where docs and code disagree, and trust the code.

Write down the ten commands you will need again (build, test, lint, run, release) so the second session does not rediscover them.
