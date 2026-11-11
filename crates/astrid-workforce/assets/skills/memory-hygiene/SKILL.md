---
name: memory-hygiene
description: Rules for keeping an agent memory store trustworthy — record what matters, summarize what accumulates, tombstone what rots.
when_to_use: Use when saving facts to memory, curating the memory store, or when recall returns stale or contradictory results.
---

# Memory hygiene

Memory is a garden, not a landfill. Tend it with the five verbs and these rules:

1. Record decisions and their reasons, not raw chatter. A memory entry should let a stranger reconstruct why we chose X over Y.
2. One topic, one note. When a topic's note keeps growing, synthesize the accumulated pages into a summary and link the sources in its frontmatter.
3. Prefer updating over duplicating: before `remember`, recall the topic first. Three half-overlapping notes become one honest note.
4. Tombstone, never delete. `forget` with a reason keeps history auditable; an unexplained gap in memory is worse than stale content.
5. Date every claim. Facts with timestamps age gracefully; facts without them silently become wrong.
6. Distrust single-source memories. When recall returns something load-bearing, verify it against the repo or a fresh source before acting on it.
7. Curate periodically: forgotten pages stay tombstoned, aliases get maintained, and entity edges point at pages that still exist.
