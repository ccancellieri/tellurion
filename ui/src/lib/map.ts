import maplibregl from 'maplibre-gl';
import type { Extent } from './api';
import { demoTileTemplateFromAdvertisedLink, isVectorDemoSource, type DemoSourceResponse } from './demo-source';
import { maplibreTileTemplate, resolveDataHref, type AdvertisedTileLink } from './tile-url';

export interface WorkspaceContext {
  tenantId: string;
  catalogId: string;
}

export interface WorkspaceLayer {
  id: string;
  title: string;
  visible: boolean;
}

export interface CatalogChoice {
  id: string;
  featuresRoot: string;
}

export interface LinkLike extends AdvertisedTileLink {}

export interface TileSetLike {
  tileMatrixSetId?: string;
  tileMatrixSetLimits?: Array<{ tileMatrix: string }>;
  mediaTypes?: string[];
  layers?: Array<{ id: string }>;
  links: LinkLike[];
}

export type WorkspaceRenderPlan =
  | {
      mode: 'vector';
      template: string;
      sourceLayers: string[];
      minzoom: number;
      maxzoom: number;
      reason?: string;
    }
  | {
      mode: 'raster';
      template: string;
      minzoom: number;
      maxzoom: number;
      reason?: string;
    }
  | { mode: 'preview'; itemsHref: string; reason?: string }
  | { mode: 'unavailable'; reason: string };

export interface WorkspaceMapIds {
  sourceId: string | null;
  layerIds: string[];
}

export interface DemoRasterMapHandoff {
  sourceId: string;
  layerId: string;
  template: string;
  extent: [number, number, number, number] | null;
  attribution: string;
}

export interface DemoVectorMapHandoff {
  sourceId: string;
  layerId: string;
  sourceLayer: string;
  template: string;
  extent: [number, number, number, number];
  attribution: string;
  geometryType: string;
}

// No basemap tiles: the demo bundle has no runtime network dependency
// beyond the Tellurion API itself, so every panel's map starts from a flat
// background rather than fetching raster/vector tiles from a third party.
const BLANK_STYLE: maplibregl.StyleSpecification = {
  version: 8,
  sources: {},
  layers: [
    {
      id: 'background',
      type: 'background',
      paint: { 'background-color': '#eef2f5' },
    },
  ],
};

export function createMap(container: HTMLElement): maplibregl.Map {
  return new maplibregl.Map({
    container,
    style: BLANK_STYLE,
    center: [0, 0],
    zoom: 1,
    attributionControl: false,
  });
}

/** Keep MapLibre's raster fetch burst inside the anonymous demo session's
 * server-advertised operation ceiling. The dedicated preview has one map, so
 * this global MapLibre limit applies only to that public-demo surface. */
export function setDemoImageRequestLimit(maxConcurrentOperations: number): void {
  maplibregl.setMaxParallelImageRequests(maxConcurrentOperations);
}

/** Frames the map on a collection's declared extent (the `extent.spatial`
 * object `/collections` reports) so a panel opens looking at real data
 * instead of an empty world view. A `null` extent (the server never
 * fabricates one — see `collection_extent`) or a degenerate single-point
 * bbox both fall back sensibly rather than throwing. */
export function fitToExtent(map: maplibregl.Map, extent: Extent | null): void {
  const bbox = extent?.spatial.bbox[0];
  if (!bbox) return;
  const [minx, miny, maxx, maxy] = bbox;
  if (minx === maxx && miny === maxy) {
    map.jumpTo({ center: [minx, miny], zoom: 10 });
    return;
  }
  map.fitBounds(
    [
      [minx, miny],
      [maxx, maxy],
    ],
    { padding: 24, animate: false },
  );
}

/** Maps only a link returned by the demo API. This function deliberately has
 * no URL-building parameters: a caller cannot turn an opaque source id into
 * a new remote route by guessing the path. */
export function demoRasterMapHandoff(
  source: DemoSourceResponse,
  origin: string,
): DemoRasterMapHandoff | null {
  const template = demoTileTemplateFromAdvertisedLink(source.links.tile_template, origin, source.id);
  if (!template || !/^[A-Za-z0-9_-]+$/.test(source.id)) return null;
  return {
    sourceId: `demo-source-${source.id}`,
    layerId: `demo-layer-${source.id}`,
    template,
    extent: source.extent,
    attribution: source.attribution,
  };
}

/** The vector handoff is still capability-bound: MapLibre receives only the
 * exact MVT template and source layer that the temporary server response
 * advertised. No remote locator reaches this boundary. */
export function demoVectorMapHandoff(
  source: DemoSourceResponse,
  origin: string,
): DemoVectorMapHandoff | null {
  if (!isVectorDemoSource(source) || !source.extent || !source.geometryType || !source.links.mvt_tile_template) return null;
  const template = demoTileTemplateFromAdvertisedLink(
    source.links.mvt_tile_template,
    origin,
    source.id,
    'mvt',
  );
  if (!template || !/^[A-Za-z0-9_-]+$/.test(source.id)) return null;
  return {
    sourceId: `demo-source-${source.id}`,
    layerId: `demo-layer-${source.id}`,
    sourceLayer: source.id,
    template,
    extent: source.extent,
    attribution: source.attribution,
    geometryType: source.geometryType,
  };
}

/** Reads the optional UI context from the current URL. The active API adapter
 * still determines which catalog is queried; this helper deliberately keeps
 * the URL state separate so the shell does not pretend that a local change
 * configured the server. */
export function contextFromSearch(
  search: string,
  defaultTenantId: string,
  defaultCatalogId: string,
): WorkspaceContext {
  const params = new URLSearchParams(search);
  const tenantId = params.get('tenant')?.trim() || defaultTenantId;
  const catalogId = params.get('catalog')?.trim() || defaultCatalogId;
  return { tenantId, catalogId };
}

export function contextSearch(context: WorkspaceContext): string {
  const params = new URLSearchParams();
  params.set('tenant', context.tenantId);
  params.set('catalog', context.catalogId);
  return `?${params.toString()}`;
}

export function linkByRel<T extends { rel: string }>(
  links: readonly T[],
  relSuffix: string,
): T | undefined {
  return links.find((link) => link.rel === relSuffix || link.rel.endsWith(`/${relSuffix}`));
}

export function catalogChoicesFromTenantLinks(
  links: readonly LinkLike[],
  documentHref: string,
  origin: string,
  tenantId: string,
): CatalogChoice[] {
  const choices: CatalogChoice[] = [];
  const seen = new Set<string>();
  for (const link of links) {
    if (link.rel !== 'features') continue;
    const featuresRoot = resolveDataHref(link.href, documentHref, origin);
    if (!featuresRoot) continue;
    try {
      const segments = new URL(featuresRoot).pathname
        .split('/')
        .filter(Boolean)
        .map((segment) => decodeURIComponent(segment));
      if (
        segments.length !== 4 ||
        segments[0] !== tenantId ||
        segments[1] !== 'features' ||
        segments[2] !== 'catalogs' ||
        !segments[3].trim() ||
        seen.has(segments[3])
      ) {
        continue;
      }
      seen.add(segments[3]);
      choices.push({ id: segments[3], featuresRoot });
    } catch {
      continue;
    }
  }
  return choices;
}

export function selectCatalog(
  choices: readonly CatalogChoice[],
  requestedId: string,
  tenantId = 'unknown',
): { choice: CatalogChoice | null; reason?: string } {
  const requested = choices.find((choice) => choice.id === requestedId);
  if (requested) return { choice: requested };
  const choice = choices[0];
  if (!choice) {
    return {
      choice: null,
      reason: `Tenant “${tenantId}” does not advertise a Features catalog.`,
    };
  }
  return {
    choice,
    reason: `Catalog “${requestedId}” is not advertised; opened “${choice.id}” instead.`,
  };
}

export function addWorkspaceLayer(
  layers: WorkspaceLayer[],
  layer: WorkspaceLayer,
): WorkspaceLayer[] {
  return layers.some((current) => current.id === layer.id) ? layers : [...layers, layer];
}

export function setWorkspaceLayerVisibility(
  layers: WorkspaceLayer[],
  id: string,
  visible: boolean,
): WorkspaceLayer[] {
  return layers.map((layer) => (layer.id === id ? { ...layer, visible } : layer));
}

export function removeWorkspaceLayer(layers: WorkspaceLayer[], id: string): WorkspaceLayer[] {
  return layers.filter((layer) => layer.id !== id);
}

export function withoutWorkspacePreview<T>(
  previews: ReadonlyMap<string, T>,
  id: string,
): Map<string, T> {
  const next = new Map(previews);
  next.delete(id);
  return next;
}

export function mergeCollectionPage<T extends { id: string }>(
  current: readonly T[],
  page: { collections: readonly T[]; links: readonly LinkLike[] },
  pageDocumentHref: string,
  origin: string,
): { collections: T[]; nextHref: string | null } {
  const collections = [...current];
  const seen = new Set(current.map((collection) => collection.id));
  for (const collection of page.collections) {
    if (seen.has(collection.id)) continue;
    seen.add(collection.id);
    collections.push(collection);
  }
  const next = linkByRel(page.links, 'next');
  return {
    collections,
    nextHref: next ? resolveDataHref(next.href, pageDocumentHref, origin) : null,
  };
}

function tileZoomRange(tileSet: TileSetLike): { minzoom: number; maxzoom: number } | null {
  if (tileSet.tileMatrixSetId !== 'WebMercatorQuad') return null;
  const rawLimits = tileSet.tileMatrixSetLimits;
  if (!rawLimits?.length) return null;
  const zooms = rawLimits.map((limit) => Number(limit.tileMatrix));
  if (zooms.some((zoom) => !Number.isInteger(zoom) || zoom < 0)) return null;
  return { minzoom: Math.min(...zooms), maxzoom: Math.max(...zooms) };
}

export function workspaceRenderPlan(input: {
  collectionLinks: readonly LinkLike[];
  collectionDocumentHref: string;
  tileSet: TileSetLike | null;
  tileSetDocumentHref: string;
  origin: string;
  fallbackReason?: string;
  excludedModes?: readonly ('vector' | 'raster')[];
}): WorkspaceRenderPlan {
  const {
    collectionLinks,
    collectionDocumentHref,
    tileSet,
    tileSetDocumentHref,
    origin,
    fallbackReason,
    excludedModes = [],
  } = input;
  if (tileSet) {
    const zoomRange = tileZoomRange(tileSet);
    if (zoomRange) {
      const sourceLayers = tileSet.layers
        ? [...new Set(tileSet.layers.map((layer) => layer.id).filter((id) => id.trim()))]
        : [];
      const mvtLink = tileSet.links.find(
        (link) =>
          link.rel === 'item' && link.type === 'application/vnd.mapbox-vector-tile',
      );
      const mvtTemplate = mvtLink
        ? maplibreTileTemplate(mvtLink, tileSetDocumentHref, origin)
        : null;
      if (
        !excludedModes.includes('vector') &&
        tileSet.mediaTypes?.includes('application/vnd.mapbox-vector-tile') &&
        mvtTemplate &&
        sourceLayers.length
      ) {
        return {
          mode: 'vector',
          template: mvtTemplate,
          sourceLayers,
          ...zoomRange,
          ...(fallbackReason ? { reason: fallbackReason } : {}),
        };
      }

      const pngLink = tileSet.links.find(
        (link) => link.rel === 'item' && link.type === 'image/png',
      );
      const pngTemplate = pngLink
        ? maplibreTileTemplate(pngLink, tileSetDocumentHref, origin)
        : null;
      if (
        !excludedModes.includes('raster') &&
        tileSet.mediaTypes?.includes('image/png') &&
        pngTemplate
      ) {
        const reason =
          fallbackReason ??
          (tileSet.mediaTypes?.includes('application/vnd.mapbox-vector-tile')
            ? sourceLayers.length
              ? 'The advertised vector tile template is unusable; using raster tiles.'
              : 'Vector tiles have no advertised source layers; using raster tiles.'
            : undefined);
        return {
          mode: 'raster',
          template: pngTemplate,
          ...zoomRange,
          ...(reason ? { reason } : {}),
        };
      }
    }
  }

  const items = linkByRel(collectionLinks, 'items');
  const itemsHref = items
    ? resolveDataHref(items.href, collectionDocumentHref, origin)
    : null;
  if (itemsHref) {
    return {
      mode: 'preview',
      itemsHref,
      ...(fallbackReason ? { reason: fallbackReason } : {}),
    };
  }
  return {
    mode: 'unavailable',
    reason: fallbackReason
      ? `${fallbackReason} No compatible tile or feature representation is advertised.`
      : 'No compatible tile or feature representation is advertised.',
  };
}

function opaqueIdPart(value: string): string {
  return [...value]
    .map((character) => character.codePointAt(0)!.toString(16))
    .join('-');
}

export function workspaceMapIds(
  collectionId: string,
  plan: WorkspaceRenderPlan,
): WorkspaceMapIds {
  if (plan.mode === 'unavailable') return { sourceId: null, layerIds: [] };
  const collectionToken = opaqueIdPart(collectionId);
  const sourceId = `workspace-source-${collectionToken}`;
  if (plan.mode === 'raster') {
    return { sourceId, layerIds: [`workspace-layer-${collectionToken}-raster`] };
  }
  if (plan.mode === 'preview') {
    return {
      sourceId,
      layerIds: ['fill', 'line', 'point'].map(
        (kind) => `workspace-layer-${collectionToken}-preview-${kind}`,
      ),
    };
  }
  return {
    sourceId,
    layerIds: plan.sourceLayers.flatMap((_, index) =>
      ['fill', 'line', 'point'].map(
        (kind) => `workspace-layer-${collectionToken}-vector-${index}-${kind}`,
      ),
    ),
  };
}

/** Allows server-advertised links that are safe to navigate from the UI.
 * Relative links must remain on this origin; absolute links are limited to
 * HTTP(S), which excludes executable and browser-internal URL schemes. */
export function safeEndpointHref(href: string, origin: string): string | null {
  try {
    const resolved = new URL(href, origin);
    if (resolved.protocol !== 'http:' && resolved.protocol !== 'https:') return null;
    const isRelative = !/^[a-z][a-z\d+.-]*:/i.test(href);
    if (isRelative && resolved.origin !== origin) return null;
    return isRelative ? href : resolved.href;
  } catch {
    return null;
  }
}
