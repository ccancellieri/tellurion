// Typed fetch helpers for the Tellurion HTTP API. Every path is
// root-absolute (`/{tenant}/features/catalogs/{catalog}/collections`, not a
// relative one) so the same code resolves correctly whether the bundle is
// served standalone from Vite or embedded by the server itself at `/ui` —
// the API always lives at the origin root, not under `/ui`.

/** Placeholder tenant/catalog external ids until this UI grows its own
 * tenant/catalog selector — every request here targets the example config's
 * `public` tenant and `default` catalog (see `config/example.yaml`). */
export const DEFAULT_TENANT_ID = 'public';
export const DEFAULT_CATALOG_ID = 'default';

export interface Link {
  href: string;
  rel: string;
  type?: string;
  title?: string;
  templated?: boolean;
}

export interface Extent {
  spatial: {
    bbox: [number, number, number, number][];
    crs: string;
  };
}

export interface CollectionSummary {
  id: string;
  title: string;
  itemType: string;
  extent: Extent | null;
  links: Link[];
}

export interface CollectionsResponse {
  links: Link[];
  collections: CollectionSummary[];
}

export interface TenantDirectoryResponse {
  tenant: string;
  links: Link[];
}

export interface FeaturesLandingResponse {
  links: Link[];
}

export interface TileSetSummary {
  links: Link[];
}

export interface TileSetListResponse {
  tilesets: TileSetSummary[];
}

export interface TileSetLayer {
  id: string;
  dataType?: string;
}

export interface TileMatrixSetLimit {
  tileMatrix: string;
  minTileRow?: number;
  maxTileRow?: number;
  minTileCol?: number;
  maxTileCol?: number;
}

export interface TileSet {
  tileMatrixSetId: string;
  tileMatrixSetLimits: TileMatrixSetLimit[];
  mediaTypes: string[];
  layers: TileSetLayer[];
  links: Link[];
}

export interface FeatureCollectionResponse {
  type: 'FeatureCollection';
  features: GeoJSON.Feature[];
  numberMatched?: number;
  numberReturned: number;
  links: Link[];
}

export interface StyleSummary {
  id: string;
  links: Link[];
}

export interface StylesListResponse {
  styles: StyleSummary[];
}

async function getJson<T>(
  path: string,
  resourceLabel: string,
  signal?: AbortSignal,
): Promise<T> {
  const response = await fetch(path, {
    headers: { Accept: 'application/json' },
    signal,
  });
  if (!response.ok) {
    throw new Error(
      `${resourceLabel} request failed: ${response.status} ${response.statusText}`,
    );
  }
  return (await response.json()) as T;
}

export function fetchTenantDirectory(
  tenantId: string,
  signal?: AbortSignal,
): Promise<TenantDirectoryResponse> {
  return getJson<TenantDirectoryResponse>(
    `/${encodeURIComponent(tenantId)}`,
    'Tenant directory',
    signal,
  );
}

export function fetchFeaturesLanding(
  href: string,
  signal?: AbortSignal,
): Promise<FeaturesLandingResponse> {
  return getJson<FeaturesLandingResponse>(href, 'Features landing page', signal);
}

export function fetchCollections(
  href: string,
  signal?: AbortSignal,
): Promise<CollectionsResponse> {
  return getJson<CollectionsResponse>(href, 'Collections', signal);
}

/** The advanced protocol lab remains a fixed demo of the example catalog.
 * The operator workspace never uses this helper: it follows the tenant
 * directory and Features landing links instead. */
export function fetchDefaultCollections(signal?: AbortSignal): Promise<CollectionsResponse> {
  return fetchCollections(
    `/${DEFAULT_TENANT_ID}/features/catalogs/${DEFAULT_CATALOG_ID}/collections`,
    signal,
  );
}

export function fetchTileSetList(
  href: string,
  signal?: AbortSignal,
): Promise<TileSetListResponse> {
  return getJson<TileSetListResponse>(href, 'TileSet list', signal);
}

export function fetchTileSet(href: string, signal?: AbortSignal): Promise<TileSet> {
  return getJson<TileSet>(href, 'TileSet', signal);
}

/** `href` is either the initial
 * `/{tenant}/features/catalogs/{catalog}/collections/{cid}/items?...` path
 * or a server-supplied `next` link — both are root-absolute, so this is the
 * one function every paging step in the features panel calls. */
export function fetchItems(
  href: string,
  signal?: AbortSignal,
): Promise<FeatureCollectionResponse> {
  return getJson<FeatureCollectionResponse>(href, 'Feature page', signal);
}

export function fetchStyles(): Promise<StylesListResponse> {
  return getJson<StylesListResponse>(
    `/${DEFAULT_TENANT_ID}/styles/catalogs/${DEFAULT_CATALOG_ID}/styles`,
    'Styles',
  );
}

export function fetchStyleDocument(styleId: string): Promise<Record<string, unknown>> {
  return getJson<Record<string, unknown>>(
    `/${DEFAULT_TENANT_ID}/styles/catalogs/${DEFAULT_CATALOG_ID}/styles/${encodeURIComponent(styleId)}`,
    'Style',
  );
}

/** Collections that expose an `items` link (i.e. `hasFeatures`, per the
 * server's `collection_summary`) — the only ones the features/vector/PNG
 * panels can meaningfully browse. */
export function collectionsWithFeatures(response: CollectionsResponse): CollectionSummary[] {
  return response.collections.filter((c) => c.links.some((l) => l.rel === 'items'));
}

/** Every listed collection has a tiles lane too (see `list_collections`'s
 * `has_tiles` filter) — even a tiles-only collection is still eligible for
 * the vector/PNG/styled/3D panels. */
export function collectionIds(response: CollectionsResponse): string[] {
  return response.collections.map((c) => c.id);
}
