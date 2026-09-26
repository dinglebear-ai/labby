---
title: "Bounded Skill Discovery - Task"
created: 2026-09-26
updated: 2026-09-26
---

# Objective

Continue the Cloud Agent / Microsandbox / Labby handoff at the source-validation gate. The original timeout hotfix b7637da is already merged as PR #816 (main 95983b2); do not duplicate it or replace production to test this follow-up.

## This implementation slice

Push Code Mode namespace restrictions ahead of Skill federation, and make a smaller SkillDiscoverRequest.max_items bound upstream page requests, retained descriptors and candidate validation work. Preserve authenticated subject scope, private Artifact access, exposure, invalidation, full operator/cache semantics and cold get/resource reads. Record latency/count/truncation metadata. Reproduce bugs before changing their implementation.

During complete product validation, also repair the independently reproduced process-global test-home race without changing production authentication or path behavior. Run the existing real-process Skill and Code Mode integration harnesses rather than relying only on mocked output.

## Acceptance boundary

Local source and real-process transport qualification is necessary but not sufficient for the handoff. A Linux promotion artifact, separate real-fleet staging, cold/warm large-catalog tests and failure/soak qualification are still required before production replacement. The existing 256/1024 per-upstream safety ceilings are not a complete Depot catalog and are not raised by this patch.
