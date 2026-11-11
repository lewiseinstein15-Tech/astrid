---
name: incident-response
description: A calm runbook for live failures — stabilize first, diagnose second, keep the record honest throughout.
when_to_use: Use when something is on fire in production, when errors spike, or when a system is behaving differently from its baseline for unknown reasons.
---

# Incident response

1. **Acknowledge and timestamp.** Note when the incident started and who is on it. An incident without a timeline cannot be learned from.
2. **Stabilize before diagnosing.** Roll back, switch to the last known-good config, shed load. Do not debug live while users bleed; mitigation has priority over understanding.
3. **Capture evidence now**: logs, dashboards, recent deploys, request samples. Evidence decays — screens rotate, logs truncate.
4. **Reduce the blast radius**: disable the affected feature rather than the whole system, queue instead of dropping, cache instead of hammering the failing dependency.
5. **Communicate on a cadence**: post a short status every 15 minutes even if the update is "still investigating". Silence breeds duplicate work.
6. **Diagnose with the evidence**: form one hypothesis, test it against the captured data, move on when falsified. No shotgun fixes.
7. **Verify the fix under the real load shape**, then keep watching past the first green dashboard.
8. **Write the postmortem within 48 hours**: timeline, root cause, what made detection slow, and numbered action items with owners. Blame is not an action item.
