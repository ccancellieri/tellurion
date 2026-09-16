# Demo evidence and dataset selection guidelines

Research reviewed: 2026-09-16. These are acceptance rules, not a claim that
candidate datasets or proposed diagnostics already work in Tellurion.

## Reuse first, then strengthen the demonstration

Preserve existing video, attribution, historical demo URLs and version identity.
Reuse verified footage as an introduction; add focused demonstrations rather
than replacing the history. Recheck destination links before publishing a new
cut. Do not label older footage as a newer release or as a benchmark.

Every new demonstration must answer a user question beyond "can it show a map?":
open a remote source without preparing a database, inspect meaningful attributes,
demonstrate bounded spatial access, or change a supported rendering option.
Show only operations available in the exact deployed build. In particular, the
stateless public preview is not the persistent administration interface and does
not currently expose user-defined styles or configurable tile caching.

## Current inventory findings

The existing [source inventory](../../demo/sources/public-examples.yaml) records:

- ESA WorldCover: a categorical 10 m land-cover map, not aerial RGB photography.
  Keep it for thematic interpretation with an appropriate legend; do not expect
  building-level photographic detail from deeper zoom.
- Monaco Open Buildings: 868 features; useful for a quick, bounded introduction,
  not evidence of large-dataset capacity.
- Natural Earth 1:110m coastline: a deliberately generalized vector dataset,
  useful for format compatibility rather than street-level detail.

These findings do not establish the cause of the reported weak raster zoom.
Source resolution, overview selection, decoder support, rendering and viewer
requests still need to be distinguished experimentally.

## Candidate research shortlist

| Source | Why investigate | Unresolved activation gates |
| --- | --- | --- |
| [LINZ imagery](https://github.com/linz/imagery) | Public imagery bucket and detailed urban aerial datasets | Pin an exact asset, dataset terms, CRS, codec, range behavior and tested map views |
| [SWISSIMAGE](https://www.swisstopo.admin.ch/en/orthoimage-swissimage-10) | RGB COG, 10 cm plains/main valleys and 25 cm Alps | EPSG:2056 and JPEG decoding/reprojection compatibility; exact asset and terms |
| [Overture Maps](https://docs.overturemaps.org/getting-data/) | Cloud-hosted GeoParquet with rich urban content | Pin release and partition; check schema, attribution, spatial pruning and geometry correctness |
| [NAIP on AWS](https://registry.opendata.aws/naip/) | Detailed RGB aerial COGs | Listed buckets are Requester Pays: not an anonymous preview default without a separately verified access route |

Provider documentation is discovery evidence, not proof that an individual object
is compatible. None of these candidates is activated by this document.

## Research deliverable for each candidate

Record publisher and authoritative source page; exact object URL; dataset version
and acquisition date; dataset-specific license and attribution; CRS, bounds,
bands, codec and native resolution; available overviews or spatial indexing;
object length and strong validator where available; and explicit unknowns.

Verify access without credentials or unexpected redirects. Use a tiny bounded
Range request and check both HTTP 206 and Content-Range, not only Accept-Ranges.
Check CORS only where the browser accesses the publisher directly; server-side
source reads are a different path. Do not weaken URL/SSRF rules to admit a sample.

Define three reproducible views: overview, neighborhood and native-detail view,
plus a short explanation of what each demonstrates. For thematic data, choose
an equivalent scale-appropriate journey instead of promising photographic detail.
Record the tested build, screenshots, errors and resource ceilings. Keep candidates
non-executable until the full journey passes. Dataset licensing is separate from
the license of a provider's software or metadata repository.

## Raster zoom acceptance

The [OGC COG standard](https://docs.ogc.org/is/21-026/21-026.html), sections
7.2 and 8, describes reduced-resolution subfiles and HTTP range support.
Internal TIFF tiles/overviews are not a one-to-one map of viewer zoom levels;
see also section 6.2. No new standards-conformance claim follows from a screenshot.

For a pinned source, record base resolution, overview dimensions and compression.
Trace viewer tile requests through rendering to the selected source resolution
and fetched byte ranges. Compare known windows with an independent decoder.
Zooming in must reveal available source detail rather than enlarge a fixed preview;
zooming beyond native resolution must not imply newly available detail.
Categorical data must preserve class meaning during resampling.

Retain reproducible tests for zoom transitions, panning, nodata, extent boundaries
and unsupported inputs, with bounded memory, concurrency, requests and temporary
disk. A prettier replacement image must not hide a decoder or overview defect.

## Make the value observable

Where instrumentation exists, distinguish browser response bytes from upstream
source bytes, metadata reads from data reads, retries from successful reads,
and cold starts from warm runs. Show source identity, native resolution, current
view and measured work in an optional diagnostic panel. Missing observations are
"unavailable", never zero or an inferred cache hit.

Report range-native access, chunk-native access, bounded archive spooling and
ingestion separately. A partial HTTP read alone does not prove spatial pruning.
Keep public-preview diagnostics session-scoped and free of secrets or other users'
activity. Do not expose global operational metrics through the anonymous demo.

Use the [benchmark methodology](../benchmarking.md) for performance claims.
For data selection, start with metadata and tiny reads, then one bounded view;
do not download countries or run full-dataset scans just to choose an example.
Keep the original small fixtures as fast correctness checks even after richer
showcase datasets are added.

## Follow-up tracking

- [Raster zoom and range-read verification (#48)](https://github.com/ccancellieri/tellurion/issues/48).
- [High-detail dataset qualification (#49)](https://github.com/ccancellieri/tellurion/issues/49).
- [Release demo acceptance (#45)](https://github.com/ccancellieri/tellurion/issues/45).
