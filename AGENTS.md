# Tellurion development instructions

Before project work, read `.agents/skills/tellurion-development/SKILL.md`
and its development guidelines. Always also load
the installed `ogc-api-standards` skill, even when the task
does not directly change an OGC endpoint. Read only the relevant standards-family
references for protocol work; GeoID's implementation map is not Tellurion's map.

Verify implementation facts against current manifests, source, and CI. Historical driver inventories,
versions, repository visibility, and source paths may be stale. Preserve current
clean-room policy: transfer ideas and observable contracts from GeoID, not code,
tests, private fixtures, or concrete manager architecture.

Version this file and the reviewed project skill. Keep private runtime context,
private issue references, credentials, infrastructure identifiers, and personal
information untracked. The skills do not authorize external writes, publication,
deployment, destructive operations, or access to private example endpoints.

Use focused regression tests and verify wider affected contracts before claiming
completion. Report skipped live tests and distinguish measured performance from
design targets. Inspect explicit remote targets before any authorized mutation.
