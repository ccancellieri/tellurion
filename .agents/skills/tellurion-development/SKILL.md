---
name: tellurion-development
description: Use when planning, implementing, reviewing, testing, or documenting Tellurion, including Rust crates, geospatial drivers, Features, tiles, rendering, caches, ingestion, and migration of ideas from GeoID.
---

# Tellurion development

## Required loading

1. Read this skill and [development guidelines](references/development-guidelines.md) before project work. Apply only the checks relevant to the task; a question or review is not authorization to implement.
2. **REQUIRED SUB-SKILL: `ogc-api-standards`.** Always load the installed `ogc-api-standards` skill, including for packaging or performance-only work. For a protocol change, load the relevant standards-family reference and fetch the normative requirements before claiming conformance. The GeoID implementation map is not a Tellurion implementation map; load it only when also working on GeoID.
3. Read current project instructions, `Cargo.toml`, toolchain configuration, and the affected crate/test boundaries. Check current CI commands. Old project notes are not evidence of today's drivers, deployment scope, or conformance.

If a required skill is unavailable, report the missing dependency, continue safe inspection, and do not claim its checks passed. Treat private examples in local references as confidential, not as test targets.

## Working contract

For each change, state the observable outcome, owning crate or interface, applicable driver capabilities, work/memory bounds, and regression evidence. Prefer a small vertical slice using existing boundaries over new platform infrastructure.

When transferring an idea from GeoID, describe the problem and public behavior independently, verify provenance and current licensing constraints, then implement against Tellurion's own contracts. Do not copy GeoID source, tests, private fixtures, or manager architecture. A historical issue is a design lead, not a requirement to reproduce the old system.

For performance work, test correctness before comparing results. Record cold and warm cache conditions separately. A global row cap is not representative low-zoom sampling and must not silently truncate exact feature queries. Cache, style, query, source revision, and access scope must remain consistent.

Finish with behavior changed, executed checks, applicable standard requirements, measurement conditions, and limitations. Do not turn a local development request into a deployment, public post, issue mutation, or license change.

## Versioned guidance and private context

Version reviewed, portable project skills and agent instructions. Keep personal runtime configuration and private context outside tracked artifacts. Do not create issue archives or retain private source text. Public material must contain no private endpoints, employee details, raw logs, credentials, or internal source references. Inspect explicit repository targets before remote writes; authentication does not grant task authorization.
