# Inspect a STAC catalog from the operator console

The operator UI includes a read-only STAC search inspector. It does not ingest
data, change catalog configuration, or automatically download assets. This
describes the source build containing the inspector, not availability in an
older release or an independently deployed demo.

## Open the correct context

1. Open the operator console at `/ui/?tenant=public&catalog=default`, replacing
   the tenant and catalog with your deployment's identifiers.
2. Expand **Advanced protocol lab**. The console reads the tenant directory
   and follows its advertised STAC catalog link.
3. Select **STAC** when it appears. The tab is available only when that catalog
   advertises a supported GET search link. If discovery fails, use **Retry STAC
   discovery**; if no GET search is advertised, the console explains this.

The route hierarchy remains
`/{tenant}/{protocol}/catalogs/{catalog}/collections/{collection}/…`.
The inspector never silently switches to another catalog, constructs a search
endpoint, or guesses a pagination cursor. Changing the tenant/catalog through
the console reloads the selected context.

## Search and inspect

An initial search requests ten items. Narrow the search using collection IDs,
item IDs (comma-separated), or a date/time or interval, then select **Search
STAC**. For example, `2026-01-01T00:00:00Z/..` selects an open-ended interval.
Filter interpretation and driver capabilities belong to the server: unsupported
queries may return an error rather than fabricated results.

Each result shows an item ID, its time or time interval, and navigable HTTP(S)
asset links. Assets open separately and may require their own authentication.
No asset is fetched until you choose a link. This is metadata inspection, not an
automatic raster preview or a map-layer creation workflow.

**Next page** follows the server's advertised GET link, preserving its opaque
query parameters. Each page replaces the previous results, rather than growing
an unbounded list. A new search or disconnect cancels the previous request;
late responses cannot overwrite newer results.

## Deliberate limits

- At most ten displayed items, sixteen examined asset entries per item, and
  256 characters per label. Truncation is reported, not presented as a complete
  inventory.
- JSON responses are limited to 2 MB, each request to fifteen seconds, and
  directory discovery to five pages.
- Programmatic requests stay on the console's origin. Redirects,
  credential-bearing URLs, executable schemes, and templated links are refused.
  Existing same-origin browser credentials are preserved; the inspector does
  not provide a separate token-entry or sign-in flow.
- Pagination requiring POST, extra headers, or a request body is not executed;
  an unsupported-next-page notice is shown.
- The STAC inspector is part of the operator console, not the anonymous
  temporary-source demo. It does not provide Records or Processes inspection.

Search-link discovery and pagination follow the
[STAC API 1.0 Item Search contract](https://github.com/radiantearth/stac-api-spec/blob/v1.0.0/item-search/README.md#link-relations).
This bounded client is not a claim of complete STAC client support or new
server conformance.
