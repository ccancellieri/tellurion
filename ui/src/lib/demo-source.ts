export type DemoSourceFormat = 'tiled-geotiff' | 'geoparquet' | 'shapefile-zip';
export type DemoSourceTransport = 'range-native' | 'bounded-zip-spool';
export type DemoVectorStyle = 'survey-ink' | 'coastline-signal';

export interface DemoSourceProvenance {
  itemId: string;
  collectionId: string | null;
  assetKey: string;
  assetLabel: string;
  mediaType: string | null;
  roles: string[];
  itemHref: string | null;
  assetDescription: string | null;
}

export interface DemoRasterMetadata {
  layout: 'gray' | 'rgb' | 'rgba' | 'palette';
  channelCount: number;
  bitsPerSample: 8;
  epsg: 4326;
  shape: [number, number];
  pixelSize: [number, number];
  levels: Array<{
    shape: [number, number];
    tileShape: [number, number];
    pixelSize: [number, number];
  }>;
  nodata: { state: 'absent' | 'unsupported' } | { state: 'value'; value: number };
  unit: string | null;
  profiles: Array<{ id: string; label: string }>;
  defaultProfile: string;
  legend: Array<{ value: number; rgba: [number, number, number, number] }>;
}

export interface DemoSourceResponse {
  id: string;
  format: DemoSourceFormat;
  transport: DemoSourceTransport;
  revision: 'strong';
  capability_state: 'ready';
  extent: [number, number, number, number] | null;
  geometryType: string | null;
  srid: number | null;
  numberMatched: number | null;
  properties: string[];
  raster?: DemoRasterMetadata;
  attribution: string;
  limits: {
    expires_in_seconds: number;
    max_live_sources: number;
    max_concurrent_operations: number;
  };
  links: {
    self_href: string;
    items_href?: string;
    item_template?: string;
    mvt_tile_template?: string;
    tile_template?: string;
    raster_profiles?: Array<{ id: string; tile_template: string }>;
  };
}

export type DemoWorkflow =
  | { phase: 'choose'; rawUrl: string }
  | { phase: 'inspect'; source: DemoSourceResponse; opacity: number; style: DemoVectorStyle; profile: string | null; provenance: DemoSourceProvenance | null }
  | { phase: 'configure'; source: DemoSourceResponse; opacity: number; style: DemoVectorStyle; profile: string | null; provenance: DemoSourceProvenance | null }
  | { phase: 'map'; source: DemoSourceResponse; opacity: number; style: DemoVectorStyle; profile: string | null; provenance: DemoSourceProvenance | null }
  | { phase: 'error'; rawUrl: string; message: string };

export type InspectedDemoWorkflow = Extract<DemoWorkflow, { phase: 'inspect' }>;
export type ConfiguredDemoWorkflow = Extract<DemoWorkflow, { phase: 'configure' }>;
export type MappedDemoWorkflow = Extract<DemoWorkflow, { phase: 'map' }>;
export type FailedDemoWorkflow = Extract<DemoWorkflow, { phase: 'error' }>;

export type UrlEligibility =
  | { ok: true; value: string }
  | { ok: false; message: string };

const MAX_DEMO_URL_LENGTH = 2048;
const SOURCE_ID = /^[A-Za-z0-9_-]+$/;
const MAX_SOURCE_ID_LENGTH = 128;
const MAX_ATTRIBUTION_LENGTH = 1024;
const MAX_PROPERTIES = 64;
const MAX_RASTER_LEVELS = 64;
const MAX_RASTER_PROFILES = 4;
const MAX_RASTER_LEGEND_ENTRIES = 256;

/** This is an early, conservative browser hint. The server remains the only
 * authority that resolves hosts and permits a remote request. */
export function eligibleDemoSourceUrl(value: string): UrlEligibility {
  const trimmed = value.trim();
  if (!trimmed || trimmed.length > MAX_DEMO_URL_LENGTH) {
    return { ok: false, message: 'Enter a short HTTPS resource address.' };
  }
  try {
    const url = new URL(trimmed);
    if (
      url.protocol !== 'https:' ||
      (url.port && url.port !== '443') ||
      url.username ||
      url.password ||
      url.search ||
      url.hash
    ) {
      return { ok: false, message: 'Use an HTTPS resource on port 443 without credentials, a query, or a fragment.' };
    }
    return { ok: true, value: url.href };
  } catch {
    return { ok: false, message: 'Enter a valid HTTPS resource address.' };
  }
}

export function isVectorDemoSource(source: DemoSourceResponse): boolean {
  return source.format === 'geoparquet' || source.format === 'shapefile-zip';
}

export function startDemoInspection(rawUrl: string, provenance: DemoSourceProvenance | null = null): {
  succeed: (source: DemoSourceResponse) => InspectedDemoWorkflow;
  fail: (status: number) => FailedDemoWorkflow;
} {
  return {
    // The raw value deliberately does not cross this boundary. The resulting
    // object holds only server-returned, opaque data.
    succeed: (source) => ({
      phase: 'inspect',
      source,
      opacity: 1,
      style: 'survey-ink',
      profile: source.raster?.defaultProfile ?? null,
      provenance,
    }),
    fail: (status) => ({
      phase: 'error',
      rawUrl,
      message: `Source inspection was not accepted (${status}). Check the source and try again.`,
    }),
  };
}

export function configureDemoSource(state: DemoWorkflow): DemoWorkflow {
  if (state.phase !== 'inspect') throw new Error('Inspect a source before configuring it.');
  return { ...state, phase: 'configure' };
}

export function setDemoOpacity(state: DemoWorkflow, opacity: number): DemoWorkflow {
  if (state.phase !== 'inspect' && state.phase !== 'configure' && state.phase !== 'map') return state;
  return { ...state, opacity: Math.max(0, Math.min(1, opacity)) };
}

export function setDemoVectorStyle(state: DemoWorkflow, style: DemoVectorStyle): DemoWorkflow {
  if (state.phase !== 'configure') return state;
  return { ...state, style };
}

export function setDemoRasterProfile(state: DemoWorkflow, profile: string): DemoWorkflow {
  if (state.phase !== 'configure' || !state.source.raster) return state;
  return state.source.raster.profiles.some((candidate) => candidate.id === profile)
    ? { ...state, profile }
    : state;
}

export function sourceForDemoRasterProfile(source: DemoSourceResponse, profile: string | null): DemoSourceResponse {
  if (!source.raster || !profile) return source;
  const link = source.links.raster_profiles?.find((candidate) => candidate.id === profile);
  return link ? { ...source, links: { ...source.links, tile_template: link.tile_template } } : source;
}

export function publishDemoMap(state: DemoWorkflow): MappedDemoWorkflow {
  if (state.phase !== 'inspect' && state.phase !== 'configure') {
    throw new Error('Inspect a source before opening its map.');
  }
  return { ...state, phase: 'map' };
}

export function resetDemoWorkflow(_: DemoWorkflow): DemoWorkflow {
  return { phase: 'choose', rawUrl: '' };
}

/** Preserves a tile template exactly as advertised by the server, while
 * rejecting anything outside the ephemeral demo route on this origin. */
export function demoTileTemplateFromAdvertisedLink(
  href: string,
  origin: string,
  expectedSourceId?: string,
  extension: 'png' | 'mvt' = 'png',
  expectedProfile?: string,
): string | null {
  if (typeof href !== 'string' || href.length > MAX_DEMO_URL_LENGTH) return null;
  try {
    const expectedOrigin = new URL(origin).origin;
    const protectedHref = href
      .replace('{z}', '__TELLURION_DEMO_Z__')
      .replace('{y}', '__TELLURION_DEMO_Y__')
      .replace('{x}', '__TELLURION_DEMO_X__');
    if (/[{}]/.test(protectedHref)) return null;
    const url = new URL(protectedHref, expectedOrigin);
    const advertisedPath = url.pathname
      .replace('__TELLURION_DEMO_Z__', '{z}')
      .replace('__TELLURION_DEMO_Y__', '{y}')
      .replace('__TELLURION_DEMO_X__', '{x}');
    const match = advertisedPath.match(
      new RegExp(`^/demo/sources/([A-Za-z0-9_-]+)/tiles/WebMercatorQuad/\\{z\\}/\\{y\\}/\\{x\\}\\.${extension}$`),
    );
    if (
      url.origin !== expectedOrigin ||
      url.username ||
      url.password ||
      (expectedProfile === undefined ? !!url.search : url.searchParams.size !== 1 || url.searchParams.get('profile') !== expectedProfile) ||
      url.hash ||
      !match ||
      (expectedSourceId !== undefined && match[1] !== expectedSourceId)
    ) {
      return null;
    }
    return url.href
      .replace('__TELLURION_DEMO_Z__', '{z}')
      .replace('__TELLURION_DEMO_Y__', '{y}')
      .replace('__TELLURION_DEMO_X__', '{x}');
  } catch {
    return null;
  }
}

function sameOriginExactRoute(href: unknown, origin: string, pathname: string): string | null {
  if (typeof href !== 'string' || href.length > MAX_DEMO_URL_LENGTH) return null;
  try {
    const url = new URL(href, new URL(origin).origin);
    return url.origin === new URL(origin).origin && !url.username && !url.password &&
      !url.search && !url.hash && url.pathname === pathname
      ? url.href
      : null;
  } catch {
    return null;
  }
}

export function demoSourceSelfHref(source: DemoSourceResponse, origin: string): string | null {
  return SOURCE_ID.test(source.id) && source.id.length <= MAX_SOURCE_ID_LENGTH
    ? sameOriginExactRoute(source.links.self_href, origin, `/demo/sources/${source.id}`)
    : null;
}

function sameOriginItemTemplate(href: unknown, origin: string, sourceId: string): string | null {
  if (typeof href !== 'string' || href.length > MAX_DEMO_URL_LENGTH) return null;
  const protectedHref = href.replace('{featureId}', '__TELLURION_DEMO_FEATURE__');
  if (/[{}]/.test(protectedHref)) return null;
  const valid = sameOriginExactRoute(
    protectedHref,
    origin,
    `/demo/sources/${sourceId}/items/__TELLURION_DEMO_FEATURE__`,
  );
  return valid?.replace('__TELLURION_DEMO_FEATURE__', '{featureId}') ?? null;
}

function isExtent(value: unknown): value is [number, number, number, number] | null {
  if (value === null) return true;
  if (!Array.isArray(value) || value.length !== 4 ||
    !value.every((coordinate) => typeof coordinate === 'number' && Number.isFinite(coordinate))) return false;
  const [minX, minY, maxX, maxY] = value;
  return minX >= -180 && maxX <= 180 && minY >= -90 && maxY <= 90 && minX < maxX && minY < maxY;
}

function propertyNames(value: unknown): value is string[] {
  return Array.isArray(value) && value.length <= MAX_PROPERTIES &&
    value.every((name) => typeof name === 'string' && !!name.trim() && name.length <= 256) &&
    new Set(value).size === value.length;
}

function positivePair(value: unknown): value is [number, number] {
  return Array.isArray(value) && value.length === 2 &&
    value.every((item) => typeof item === 'number' && Number.isFinite(item) && item > 0);
}

function byte(value: unknown): value is number {
  return Number.isInteger(value) && (value as number) >= 0 && (value as number) <= 255;
}

function validateRasterMetadata(value: unknown): DemoRasterMetadata | null {
  if (!value || typeof value !== 'object') return null;
  const raster = value as Record<string, unknown>;
  const layout = raster.layout;
  const expectedChannels = layout === 'gray' || layout === 'palette' ? 1 : layout === 'rgb' ? 3 : layout === 'rgba' ? 4 : 0;
  if (!expectedChannels || raster.channel_count !== expectedChannels || raster.bits_per_sample !== 8 || raster.epsg !== 4326 ||
      !positivePair(raster.shape) || !raster.shape.every(Number.isInteger) || !positivePair(raster.pixel_size)) return null;
  if (!Array.isArray(raster.levels) || raster.levels.length < 1 || raster.levels.length > MAX_RASTER_LEVELS) return null;
  const levels = raster.levels.map((value) => {
    if (!value || typeof value !== 'object') return null;
    const level = value as Record<string, unknown>;
    if (!positivePair(level.shape) || !level.shape.every(Number.isInteger) ||
        !positivePair(level.tile_shape) || !level.tile_shape.every(Number.isInteger) || !positivePair(level.pixel_size)) return null;
    return { shape: [...level.shape] as [number, number], tileShape: [...level.tile_shape] as [number, number], pixelSize: [...level.pixel_size] as [number, number] };
  });
  if (levels.some((level) => !level)) return null;
  if (!raster.nodata || typeof raster.nodata !== 'object') return null;
  const rawNodata = raster.nodata as Record<string, unknown>;
  const nodata: DemoRasterMetadata['nodata'] | null = rawNodata.state === 'value' && byte(rawNodata.value)
    ? { state: 'value', value: rawNodata.value }
    : (rawNodata.state === 'absent' || rawNodata.state === 'unsupported') && rawNodata.value === undefined
      ? { state: rawNodata.state }
      : null;
  if (!nodata || (raster.unit !== undefined && (typeof raster.unit !== 'string' || !raster.unit.trim() || raster.unit.length > 128))) return null;
  if (!Array.isArray(raster.profiles) || raster.profiles.length < 1 || raster.profiles.length > MAX_RASTER_PROFILES) return null;
  const profiles = raster.profiles.map((value) => {
    if (!value || typeof value !== 'object') return null;
    const profile = value as Record<string, unknown>;
    return typeof profile.id === 'string' && /^[a-z0-9-]+$/.test(profile.id) && profile.id.length <= 32 &&
      typeof profile.label === 'string' && !!profile.label.trim() && profile.label.length <= 64
      ? { id: profile.id, label: profile.label }
      : null;
  });
  if (profiles.some((profile) => !profile) || new Set(profiles.map((profile) => profile!.id)).size !== profiles.length ||
      typeof raster.default_profile !== 'string' || !profiles.some((profile) => profile!.id === raster.default_profile)) return null;
  const expectedProfiles = layout === 'gray'
    ? ['grayscale', 'viridis']
    : layout === 'palette'
      ? ['embedded-palette']
      : ['native-rgb'];
  if (profiles.length !== expectedProfiles.length ||
      profiles.some((profile, index) => profile!.id !== expectedProfiles[index]) ||
      raster.default_profile !== expectedProfiles[0] ||
      ((layout === 'rgb' || layout === 'rgba') && nodata.state === 'value')) return null;
  if (!Array.isArray(raster.legend) || raster.legend.length > MAX_RASTER_LEGEND_ENTRIES) return null;
  const legend = raster.legend.map((value) => {
    if (!value || typeof value !== 'object') return null;
    const entry = value as Record<string, unknown>;
    return byte(entry.value) && Array.isArray(entry.rgba) && entry.rgba.length === 4 && entry.rgba.every(byte)
      ? { value: entry.value, rgba: [...entry.rgba] as [number, number, number, number] }
      : null;
  });
  if (legend.some((entry) => !entry) || new Set(legend.map((entry) => entry!.value)).size !== legend.length ||
      (layout === 'palette' ? legend.length !== 256 || legend.some((entry, index) => entry!.value !== index) : legend.length !== 0)) return null;
  return {
    layout: layout as DemoRasterMetadata['layout'], channelCount: expectedChannels, bitsPerSample: 8, epsg: 4326,
    shape: [...raster.shape] as [number, number], pixelSize: [...raster.pixel_size] as [number, number],
    levels: levels as DemoRasterMetadata['levels'], nodata, unit: typeof raster.unit === 'string' ? raster.unit : null,
    profiles: profiles as DemoRasterMetadata['profiles'], defaultProfile: raster.default_profile, legend: legend as DemoRasterMetadata['legend'],
  };
}

/** Validates the entire public-demo DTO before it can reach UI state. The
 * response id and every server route form one inseparable capability. */
export function validateDemoSourceResponse(value: unknown, origin: string): DemoSourceResponse | null {
  if (!value || typeof value !== 'object') return null;
  const response = value as Record<string, unknown>;
  const id = response.id;
  if (
    typeof id !== 'string' || id.length > MAX_SOURCE_ID_LENGTH || !SOURCE_ID.test(id) ||
    (response.format !== 'tiled-geotiff' && response.format !== 'geoparquet' && response.format !== 'shapefile-zip') ||
    (response.transport !== 'range-native' && response.transport !== 'bounded-zip-spool') ||
    response.revision !== 'strong' || response.capability_state !== 'ready' ||
    typeof response.attribution !== 'string' || !response.attribution.trim() ||
    response.attribution.length > MAX_ATTRIBUTION_LENGTH || /[<>\u0000-\u001f\u007f]/.test(response.attribution) || !isExtent(response.extent) ||
    !response.limits || typeof response.limits !== 'object' || !response.links || typeof response.links !== 'object'
  ) return null;
  const limits = response.limits as Record<string, unknown>;
  if (
    !Number.isInteger(limits.expires_in_seconds) || (limits.expires_in_seconds as number) < 1 || (limits.expires_in_seconds as number) > 15 * 60 ||
    !Number.isInteger(limits.max_live_sources) || (limits.max_live_sources as number) < 1 || (limits.max_live_sources as number) > 3 ||
    !Number.isInteger(limits.max_concurrent_operations) || (limits.max_concurrent_operations as number) < 1 || (limits.max_concurrent_operations as number) > 2
  ) return null;
  const links = response.links as Record<string, unknown>;
  const selfHref = sameOriginExactRoute(links.self_href, origin, `/demo/sources/${id}`);
  if (!selfHref) return null;

  const vector = response.format === 'geoparquet' || response.format === 'shapefile-zip';
  const expectedTransport = response.format === 'shapefile-zip' ? 'bounded-zip-spool' : 'range-native';
  if (response.transport !== expectedTransport || !propertyNames(response.properties)) return null;
  if (!vector) {
    const raster = validateRasterMetadata(response.raster);
    if (!raster || response.geometry_type !== null || response.srid !== null || response.number_matched !== null || response.properties.length !== 0 ||
        !Array.isArray(links.raster_profiles) || links.raster_profiles.length !== raster.profiles.length) return null;
    const profileLinks = links.raster_profiles.map((value) => {
      if (!value || typeof value !== 'object') return null;
      const link = value as Record<string, unknown>;
      const profile = raster.profiles.find((candidate) => candidate.id === link.id);
      const tileTemplate = profile
        ? demoTileTemplateFromAdvertisedLink(link.tile_template as string, origin, id, 'png', profile.id)
        : null;
      return profile && tileTemplate ? { id: profile.id, tile_template: tileTemplate } : null;
    });
    if (profileLinks.some((link) => !link) || new Set(profileLinks.map((link) => link!.id)).size !== raster.profiles.length) return null;
    const pngTemplate = demoTileTemplateFromAdvertisedLink(links.tile_template as string, origin, id, 'png', raster.defaultProfile);
    const defaultLink = profileLinks.find((link) => link!.id === raster.defaultProfile);
    if (!pngTemplate || defaultLink?.tile_template !== pngTemplate) return null;
    return {
      id, format: response.format, transport: response.transport, revision: 'strong', capability_state: 'ready',
      extent: response.extent, geometryType: null, srid: null, numberMatched: null, properties: [],
      raster, attribution: response.attribution,
      limits: limits as DemoSourceResponse['limits'],
      links: { self_href: selfHref, tile_template: pngTemplate, raster_profiles: profileLinks as NonNullable<DemoSourceResponse['links']['raster_profiles']> },
    };
  }
  if (
    response.raster !== undefined || links.raster_profiles !== undefined ||
    typeof response.geometry_type !== 'string' || !response.geometry_type.trim() || response.geometry_type.length > 128 ||
    response.srid !== 4326 || !Number.isInteger(response.number_matched) || (response.number_matched as number) < 0 ||
    !response.extent
  ) return null;
  const itemsHref = sameOriginExactRoute(links.items_href, origin, `/demo/sources/${id}/items`);
  const itemTemplate = sameOriginItemTemplate(links.item_template, origin, id);
  const mvtTemplate = demoTileTemplateFromAdvertisedLink(links.mvt_tile_template as string, origin, id, 'mvt');
  const pngTemplate = demoTileTemplateFromAdvertisedLink(links.tile_template as string, origin, id, 'png');
  if (!itemsHref || !itemTemplate || !mvtTemplate || !pngTemplate) return null;
  return {
    id, format: response.format, transport: response.transport, revision: 'strong', capability_state: 'ready',
    extent: response.extent as [number, number, number, number], geometryType: response.geometry_type, srid: 4326,
    numberMatched: response.number_matched as number, properties: [...response.properties], attribution: response.attribution,
    limits: limits as DemoSourceResponse['limits'],
    links: {
      self_href: selfHref, items_href: itemsHref, item_template: itemTemplate,
      mvt_tile_template: mvtTemplate, tile_template: pngTemplate,
    },
  };
}
