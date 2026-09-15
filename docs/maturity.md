# Tellurion 0.5.0-rc.1 maturity

Tellurion 0.5.0-rc.1 is a release candidate, not an announced stable release. This page
separates demonstrated capabilities from preview work so an evaluator can choose an
appropriate path without inferring a hosted service or production commitment.

| Area | Status | Appropriate use today |
|---|---|---|
| GeoPackage and PostGIS serving data plane | Stabilising | Local evaluation, integration testing, and design-partner workloads with the documented limits |
| Features, vector tiles, raster tiles, styles, search, and 3D routes | Stabilising | Capability evaluation through the documented examples and tests |
| Optional direct-read drivers | Format-specific | Use only the capability advertised by each driver; read-only does not imply write support |
| Dynamic multi-tenant control plane | Preview | Administrative workflow evaluation, not an assurance of production support |
| Stateless remote-source browser | Preview | Short-lived inspection of curated public objects within its documented security budgets |
| Tellurion Cloud | Not offered | Install and operate Tellurion on infrastructure you control |
| Commercial support or SLA | Not offered by this repository | Discuss a separately executed agreement before relying on either |

The [public demo guide](quickstart/public-demo.md) names the three remote formats that
are currently executable in that bounded browser. Candidate formats shown in a user
interface are not capabilities until a driver, test, and verified example all exist.

A stable release requires all publication checks, supported build targets, licence
surfaces, release artifacts, checksums, attestations, and the public evaluation journey
to pass at the same immutable commit. A green demonstration is evidence for that
demonstrated path; it is not an availability SLA or a benchmark result.
