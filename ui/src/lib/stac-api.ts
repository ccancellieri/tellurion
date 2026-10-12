import { DEFAULT_CATALOG_ID, DEFAULT_TENANT_ID } from './api';

export interface StacEndpoint { tenantId: string; catalogId: string; searchHref: string }
export interface StacFilters { collections?: string; ids?: string; datetime?: string }
export type StacAssetPreviewFormat = 'cog' | 'geoparquet' | 'shapefile-zip';
export interface StacAssetPreview {
  url: string;
  format: StacAssetPreviewFormat;
  itemId: string;
  collectionId: string | null;
  assetKey: string;
  assetLabel: string;
  mediaType: string | null;
  roles: string[];
  itemHref: string | null;
  assetDescription: string | null;
}
export type StacAssetPreviewDecision =
  | { state: 'eligible'; descriptor: StacAssetPreview }
  | { state: 'unavailable'; reason: string };
export interface StacAsset {
  key: string;
  title: string;
  navigationHref: string | null;
  mediaType: string | null;
  roles: string[];
  preview: StacAssetPreviewDecision;
}
export interface StacItem {
  id: string;
  title: string | null;
  collectionId: string | null;
  itemHref: string | null;
  time: string;
  assets: StacAsset[];
}
export interface StacPage { items: StacItem[]; nextHref: string | null; unsupportedNext: boolean; truncated: boolean }
type Document = Record<string, unknown>;

const MAX_LABEL_LENGTH = 256;
const MAX_ATTRIBUTION_LENGTH = 1024;
const MAX_PREVIEW_ROLES = 8;
const MAX_ROLE_LENGTH = 128;

function record(value: unknown): value is Document {
  return !!value && typeof value === 'object' && !Array.isArray(value);
}

export function stacContext(search: string): { tenantId: string; catalogId: string } {
  const params = new URLSearchParams(search);
  const tenantId = (params.get('tenant') ?? DEFAULT_TENANT_ID).trim();
  const catalogId = (params.get('catalog') ?? DEFAULT_CATALOG_ID).trim();
  if (!tenantId || !catalogId || tenantId.length > 256 || catalogId.length > 256) throw new Error('Invalid STAC context');
  return { tenantId, catalogId };
}

function safeHref(value: unknown, base: string, origin?: string): string | null {
  if (typeof value !== 'string' || value.length > 8192 || /[{}]/.test(value)) return null;
  try {
    const url = new URL(value, base);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.hash) return null;
    if (origin && url.origin !== new URL(origin).origin) return null;
    return url.href;
  } catch { return null; }
}

function previewFormatFromMediaType(value: string): StacAssetPreviewFormat | null {
  const base = value.toLowerCase().split(';', 1)[0].trim();
  if (['image/tiff', 'image/geotiff', 'application/geotiff'].includes(base)) return 'cog';
  if (['application/vnd.apache.parquet', 'application/x-parquet'].includes(base)) return 'geoparquet';
  if (['application/zip', 'application/x-zip-compressed'].includes(base)) return 'shapefile-zip';
  return null;
}

function previewFormatFromPath(url: URL): StacAssetPreviewFormat | null {
  const path = url.pathname.toLowerCase();
  if (path.endsWith('.tif') || path.endsWith('.tiff')) return 'cog';
  if (path.endsWith('.parquet')) return 'geoparquet';
  if (path.endsWith('.zip')) return 'shapefile-zip';
  return null;
}

function strictPreviewUrl(value: string): URL | null {
  try {
    const url = new URL(value);
    return url.protocol === 'https:' && (!url.port || url.port === '443') &&
      !url.username && !url.password && !url.search && !url.hash
      ? url
      : null;
  } catch {
    return null;
  }
}

function previewDecision(
  navigationHref: string | null,
  mediaType: string | null,
  descriptor: Omit<StacAssetPreview, 'url' | 'format'>,
): StacAssetPreviewDecision {
  if (!navigationHref) {
    return { state: 'unavailable', reason: 'Preview requires a safe HTTP(S) asset address without credentials or a fragment.' };
  }
  const previewUrl = strictPreviewUrl(navigationHref);
  if (!previewUrl) {
    return { state: 'unavailable', reason: 'Preview requires an unsigned HTTPS URL on port 443 without credentials or a fragment.' };
  }
  if (previewUrl.pathname.toLowerCase().endsWith('.pmtiles') ||
      mediaType?.toLowerCase().split(';', 1)[0].trim() === 'application/vnd.pmtiles') {
    return { state: 'unavailable', reason: 'Preview is unavailable because the asset format is not recognized.' };
  }
  const mediaFormat = mediaType === null ? null : previewFormatFromMediaType(mediaType);
  const extensionFormat = previewFormatFromPath(previewUrl);
  if (mediaType !== null && mediaFormat === null) {
    return { state: 'unavailable', reason: 'Preview is unavailable because the supplied media type is not recognized.' };
  }
  if (mediaFormat && extensionFormat && mediaFormat !== extensionFormat) {
    return { state: 'unavailable', reason: 'Media type and file extension identify different supported formats.' };
  }
  const format = mediaFormat ?? extensionFormat;
  if (!format) {
    return { state: 'unavailable', reason: 'Preview is unavailable because the asset format is not recognized.' };
  }
  return { state: 'eligible', descriptor: { ...descriptor, url: previewUrl.href, format } };
}

export function validateStacAssetPreview(value: unknown): StacAssetPreview | null {
  if (!record(value)) return null;
  const url = typeof value.url === 'string' ? strictPreviewUrl(value.url) : null;
  const format = value.format;
  const mediaType = value.mediaType;
  const itemHref = value.itemHref;
  const assetDescription = value.assetDescription;
  if (!url || (format !== 'cog' && format !== 'geoparquet' && format !== 'shapefile-zip') ||
      typeof value.itemId !== 'string' || !value.itemId.trim() || value.itemId.length > MAX_LABEL_LENGTH ||
      (value.collectionId !== null && (typeof value.collectionId !== 'string' || !value.collectionId.trim() || value.collectionId.length > MAX_LABEL_LENGTH)) ||
      typeof value.assetKey !== 'string' || !value.assetKey.trim() || value.assetKey.length > MAX_LABEL_LENGTH ||
      typeof value.assetLabel !== 'string' || !value.assetLabel.trim() || value.assetLabel.length > MAX_LABEL_LENGTH ||
      (mediaType !== null && (typeof mediaType !== 'string' || !mediaType.trim() || mediaType.length > MAX_LABEL_LENGTH)) ||
      !Array.isArray(value.roles) || value.roles.length > MAX_PREVIEW_ROLES ||
      !value.roles.every((role) => typeof role === 'string' && !!role.trim() && role.length <= MAX_ROLE_LENGTH) ||
      (itemHref !== null && safeHref(itemHref, url.href) !== itemHref) ||
      (assetDescription !== null && (typeof assetDescription !== 'string' || !assetDescription.trim() || assetDescription.length > MAX_ATTRIBUTION_LENGTH))) {
    return null;
  }
  const classified = previewDecision(url.href, mediaType as string | null, {
    itemId: value.itemId,
    collectionId: value.collectionId as string | null,
    assetKey: value.assetKey,
    assetLabel: value.assetLabel,
    mediaType: mediaType as string | null,
    roles: [...value.roles] as string[],
    itemHref: itemHref as string | null,
    assetDescription: assetDescription as string | null,
  });
  return classified.state === 'eligible' && classified.descriptor.format === format
    ? classified.descriptor
    : null;
}

function links(document: Document): Document[] {
  if (!Array.isArray(document.links)) return [];
  if (document.links.length > 1000) throw new Error('STAC document link limit exceeded');
  return document.links.filter(record);
}

function getLink(link: Document, base: string, origin: string): string | null {
  if ((link.method !== undefined && link.method !== 'GET') || link.templated ||
      link.headers !== undefined || link.body !== undefined || link.merge === true) return null;
  return safeHref(link.href, base, origin);
}

async function readDocument(href: string, origin: string, signal?: AbortSignal): Promise<Document> {
  const deadline = new AbortController();
  const timer = setTimeout(() => deadline.abort(), 15_000);
  const requestSignal = signal ? AbortSignal.any([signal, deadline.signal]) : deadline.signal;
  try {
    return await readDocumentWithSignal(href, origin, requestSignal);
  } finally { clearTimeout(timer); }
}

async function readDocumentWithSignal(href: string, origin: string, signal: AbortSignal): Promise<Document> {
  if (!safeHref(href, origin, origin)) throw new Error('Unsafe STAC link');
  const response = await fetch(href, { headers: { Accept: 'application/geo+json, application/json' },
    credentials: 'same-origin', redirect: 'error', signal });
  if (!response.ok) throw new Error('STAC request failed');
  if (!response.body) throw new Error('Empty STAC response');
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let bytes = 0;
  let text = '';
  try {
    while (true) {
      const part = await reader.read();
      if (part.done) break;
      bytes += part.value.byteLength;
      if (bytes > 2_000_000) { await reader.cancel(); throw new Error('STAC response size limit exceeded'); }
      text += decoder.decode(part.value, { stream: true });
    }
  } finally { reader.releaseLock(); }
  const value: unknown = JSON.parse(text + decoder.decode());
  if (!record(value)) throw new Error('Invalid STAC document');
  return value;
}

// The tenant directory's advertised protocol roots carry the catalog identity.
// Keep this route-shape recognition separate from search and pagination URLs.
function matchesCatalog(href: string, tenantId: string, catalogId: string): boolean {
  try {
    const parts = new URL(href).pathname.split('/').filter(Boolean).map(decodeURIComponent);
    return parts.length === 4 && parts[0] === tenantId && parts[1] === 'stac' && parts[2] === 'catalogs' && parts[3] === catalogId;
  } catch { return false; }
}

export async function discoverStac(search: string, origin: string, signal?: AbortSignal): Promise<StacEndpoint | null> {
  const context = stacContext(search);
  let directoryHref = new URL(`/${encodeURIComponent(context.tenantId)}`, origin).href;
  const visited = new Set<string>();
  for (let page = 0; page < 5; page += 1) {
    if (visited.has(directoryHref)) throw new Error('STAC directory pagination loop');
    visited.add(directoryHref);
    const directory = await readDocument(directoryHref, origin, signal);
    const directoryLinks = links(directory);
    for (const link of directoryLinks) {
      if (link.rel !== 'stac') continue;
      const href = getLink(link, directoryHref, origin);
      if (!href || !matchesCatalog(href, context.tenantId, context.catalogId)) continue;
      const landing = await readDocument(href, origin, signal);
      for (const candidate of links(landing)) {
        if (candidate.rel !== 'search' || candidate.type !== 'application/geo+json') continue;
        const searchHref = getLink(candidate, href, origin);
        if (searchHref) return { ...context, searchHref };
      }
      return null;
    }
    const next = directoryLinks.find((link) => link.rel === 'next');
    if (!next) return null;
    const href = getLink(next, directoryHref, origin);
    if (!href) throw new Error('Unsupported STAC directory pagination');
    directoryHref = href;
  }
  throw new Error('STAC directory page limit exceeded');
}

export function stacSearchHref(href: string, filters: StacFilters): string {
  const url = new URL(href);
  url.searchParams.set('limit', '10');
  for (const name of ['collections', 'ids', 'datetime'] as const) {
    const value = filters[name]?.trim();
    if (value && value.length > 1024) throw new Error('STAC filter too long');
    if (value) url.searchParams.set(name, value);
    else url.searchParams.delete(name);
  }
  return url.href;
}

export async function fetchStacPage(href: string, origin: string, signal?: AbortSignal): Promise<StacPage> {
  const document = await readDocument(href, origin, signal);
  if (document.type !== 'FeatureCollection' || !Array.isArray(document.features) || !document.features.every(record)) {
    throw new Error('Invalid STAC item page');
  }
  let truncated = document.features.length > 10;
  const boundedText = (value: unknown, fallback: string, maxLength = MAX_LABEL_LENGTH): string => {
    const text = typeof value === 'string' || typeof value === 'number' ? String(value) : fallback;
    if (text.length <= maxLength) return text;
    truncated = true;
    return `${text.slice(0, maxLength - 1)}…`;
  };
  const items = document.features.slice(0, 10).map((feature): StacItem => {
    const properties = record(feature.properties) ? feature.properties : {};
    const id = (typeof feature.id === 'string' || typeof feature.id === 'number') && String(feature.id).trim()
      ? boundedText(feature.id, 'ID not provided')
      : 'ID not provided';
    const title = typeof properties.title === 'string' && properties.title.trim()
      ? boundedText(properties.title, '')
      : null;
    const collectionId = typeof feature.collection === 'string' && feature.collection.trim()
      ? boundedText(feature.collection, '')
      : null;
    const time = typeof properties.datetime === 'string' ? boundedText(properties.datetime, '')
      : typeof properties.start_datetime === 'string' && typeof properties.end_datetime === 'string'
        ? `${boundedText(properties.start_datetime, '')} / ${boundedText(properties.end_datetime, '')}` : 'Time not provided';
    const assets: StacItem['assets'] = [];
    const self = links(feature).find((link) => link.rel === 'self');
    const itemHref = self ? safeHref(self.href, href) : null;
    const assetBase = itemHref ?? href;
    if (record(feature.assets)) {
      let inspected = 0;
      for (const name in feature.assets) {
        if (!Object.hasOwn(feature.assets, name)) continue;
        if (inspected++ === 16) { truncated = true; break; }
        const asset = feature.assets[name];
        if (!record(asset)) continue;
        const key = name.trim() ? boundedText(name, 'Asset') : 'Asset';
        const mediaType = typeof asset.type === 'string' && asset.type.trim()
          ? boundedText(asset.type, '')
          : null;
        const roles = Array.isArray(asset.roles)
          ? asset.roles.filter((role): role is string => typeof role === 'string' && !!role.trim())
            .slice(0, MAX_PREVIEW_ROLES).map((role) => boundedText(role, '', MAX_ROLE_LENGTH))
          : [];
        if (Array.isArray(asset.roles) && asset.roles.length > MAX_PREVIEW_ROLES) truncated = true;
        const title = boundedText(asset.title, key);
        const navigationHref = safeHref(asset.href, assetBase);
        const assetDescription = typeof asset.description === 'string' && asset.description.trim()
          ? boundedText(asset.description, '', MAX_ATTRIBUTION_LENGTH)
          : null;
        const descriptor = {
          itemId: id, collectionId, assetKey: key, assetLabel: title, mediaType, roles,
          itemHref, assetDescription,
        };
        assets.push({
          key, title, navigationHref, mediaType, roles,
          preview: previewDecision(navigationHref, mediaType, descriptor),
        });
      }
    }
    return { id, title, collectionId, itemHref, time, assets };
  });
  const nextLinks = links(document).filter((link) => link.rel === 'next');
  const nextHref = nextLinks.map((link) => getLink(link, href, origin)).find((value) => value !== null) ?? null;
  return { items, nextHref, unsupportedNext: nextLinks.length > 0 && !nextHref, truncated };
}
