import { parse } from 'yaml';
import publicExamplesYaml from '../../../demo/sources/public-examples.yaml?raw';
import { eligibleDemoSourceUrl } from './demo-source';

export type DemoFormat =
  | 'tiled-geotiff'
  | 'geoparquet'
  | 'shapefile-zip'
  | 'geojson'
  | 'geozarr'
  | 'grib2'
  | 'hdf5'
  | 'netcdf'
  | 'geopackage'
  | 'flatgeobuf'
  | 'pmtiles';

export interface DemoFixture {
  id: string;
  status: 'active' | 'candidate';
  executable: boolean;
  title: string;
  provider: string;
  license: { verification: 'confirmed' | 'review-required'; label: string; termsUrl?: string };
  attribution: string;
  sourcePage: string;
  url: string;
  transport: 'range-native' | 'chunk-native' | 'bounded-spool';
  format: DemoFormat;
  connector: { state: 'ready' | 'planned'; reason: string };
  activation: { state: string; reason: string };
  content: { kind: 'object' | 'prefix'; revision: string; expectedLength?: number; expectedStrongEtag?: string };
  resource?: {
    selected: string;
    crs: string;
    extent: [number, number, number, number];
    geometryType?: string;
    featureCount?: number;
    attributes?: string[];
    testedInitialView?: string;
  };
  render?: { profile: 'categorical-land-cover' | 'vector-default'; band?: number };
}

type UnknownRecord = Record<string, unknown>;
const formats = new Set<DemoFormat>(['tiled-geotiff', 'geoparquet', 'shapefile-zip', 'geojson', 'geozarr', 'grib2', 'hdf5', 'netcdf', 'geopackage', 'flatgeobuf', 'pmtiles']);
const transports = new Set<DemoFixture['transport']>(['range-native', 'chunk-native', 'bounded-spool']);
const statuses = new Set<DemoFixture['status']>(['active', 'candidate']);
const connectorStates = new Set<DemoFixture['connector']['state']>(['ready', 'planned']);
const licenseStates = new Set<DemoFixture['license']['verification']>(['confirmed', 'review-required']);
const contentKinds = new Set<DemoFixture['content']['kind']>(['object', 'prefix']);
const activationStates = new Set(['approved', 'review-blocked', 'connector-unavailable']);
const MAX_FIXTURE_TEXT_LENGTH = 4096;
const vectorFormats = new Set<DemoFormat>(['geoparquet', 'shapefile-zip']);

function record(value: unknown, field: string): UnknownRecord {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`Invalid ${field} fixture field.`);
  return value as UnknownRecord;
}

function text(value: unknown, field: string): string {
  if (typeof value !== 'string' || !value.trim() || value.length > MAX_FIXTURE_TEXT_LENGTH) throw new Error(`Invalid ${field} fixture field.`);
  return value;
}

function httpsMetadataUrl(value: unknown, field: string): string {
  const raw = text(value, field);
  try {
    const parsed = new URL(raw);
    if (
      parsed.protocol !== 'https:' ||
      (parsed.port && parsed.port !== '443') ||
      parsed.username ||
      parsed.password
    ) throw new Error();
    return parsed.href;
  } catch {
    throw new Error(`Invalid ${field} fixture field.`);
  }
}

function oneOf<T extends string>(value: unknown, allowed: ReadonlySet<T>, field: string): T {
  const literal = text(value, field);
  if (!allowed.has(literal as T)) throw new Error(`Invalid ${field} fixture field.`);
  return literal as T;
}

function requiredBoolean(value: unknown, field: string): boolean {
  if (typeof value !== 'boolean') throw new Error(`Invalid ${field} fixture field.`);
  return value;
}

function optionalNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function optionalText(value: unknown, field: string): string | undefined {
  return value === undefined ? undefined : text(value, field);
}

function attributes(value: unknown): string[] | undefined {
  if (value === undefined) return undefined;
  if (!Array.isArray(value) || !value.length || value.length > 64) {
    throw new Error('Invalid resource attributes fixture field.');
  }
  const parsed = value.map((item) => text(item, 'resource attributes'));
  if (new Set(parsed).size !== parsed.length) {
    throw new Error('Invalid resource attributes fixture field.');
  }
  return parsed;
}

function extent(value: unknown): [number, number, number, number] {
  if (!Array.isArray(value) || value.length !== 4 || value.some((item) => typeof item !== 'number' || !Number.isFinite(item))) {
    throw new Error('Invalid resource extent fixture field.');
  }
  const result = value as [number, number, number, number];
  if (result[0] < -180 || result[2] > 180 || result[1] < -90 || result[3] > 90 || result[0] >= result[2] || result[1] >= result[3]) {
    throw new Error('Invalid resource extent fixture field.');
  }
  return result;
}

function fixture(value: unknown): DemoFixture {
  const item = record(value, 'example');
  const license = record(item.license, 'license');
  const connector = record(item.connector, 'connector');
  const activation = record(item.activation, 'activation');
  const content = record(item.content, 'content');
  const rawResource = item.resource === undefined ? undefined : record(item.resource, 'resource');
  const rawRender = item.render === undefined ? undefined : record(item.render, 'render');
  const termsUrl = license.terms_url;
  const parsed: DemoFixture = {
    id: text(item.id, 'id'),
    status: oneOf(item.status, statuses, 'status'),
    executable: requiredBoolean(item.executable, 'executable'),
    title: text(item.title, 'title'),
    provider: text(item.provider, 'provider'),
    license: {
      verification: oneOf(license.verification, licenseStates, 'license verification'),
      label: text(license.label, 'license label'),
      ...(typeof termsUrl === 'string' ? { termsUrl: httpsMetadataUrl(termsUrl, 'license terms URL') } : {}),
    },
    attribution: text(item.attribution, 'attribution'),
    sourcePage: httpsMetadataUrl(item.source_page, 'source page'),
    url: text(item.url, 'url'),
    transport: oneOf(item.transport, transports, 'transport'),
    format: oneOf(item.format, formats, 'format'),
    connector: { state: oneOf(connector.state, connectorStates, 'connector state'), reason: text(connector.reason, 'connector reason') },
    activation: { state: oneOf(activation.state, activationStates, 'activation state'), reason: text(activation.reason, 'activation reason') },
    content: {
      kind: oneOf(content.kind, contentKinds, 'content kind'),
      revision: text(content.revision, 'content revision'),
      ...(optionalNumber(content.expected_length) !== undefined ? { expectedLength: optionalNumber(content.expected_length) } : {}),
      ...(typeof content.expected_strong_etag === 'string' ? { expectedStrongEtag: content.expected_strong_etag } : {}),
    },
    ...(rawResource ? {
      resource: {
        selected: text(rawResource.selected, 'resource selected'),
        crs: text(rawResource.crs, 'resource crs'),
        extent: extent(rawResource.extent),
        ...(optionalText(rawResource.geometry_type, 'resource geometry type') !== undefined
          ? { geometryType: optionalText(rawResource.geometry_type, 'resource geometry type') }
          : {}),
        ...(optionalNumber(rawResource.feature_count) !== undefined
          ? { featureCount: optionalNumber(rawResource.feature_count) }
          : {}),
        ...(attributes(rawResource.attributes) !== undefined
          ? { attributes: attributes(rawResource.attributes) }
          : {}),
        ...(optionalText(rawResource.tested_initial_view, 'resource tested initial view') !== undefined
          ? { testedInitialView: optionalText(rawResource.tested_initial_view, 'resource tested initial view') }
          : {}),
      },
    } : {}),
    ...(rawRender ? {
      render: {
        profile: oneOf(rawRender.profile, new Set(['categorical-land-cover', 'vector-default'] as const), 'render profile'),
        ...(optionalNumber(rawRender.band) !== undefined ? { band: optionalNumber(rawRender.band) } : {}),
      },
    } : {}),
  };
  if (parsed.render?.profile === 'categorical-land-cover' && (!Number.isInteger(parsed.render.band) || (parsed.render.band ?? 0) < 1)) throw new Error('Invalid render band fixture field.');
  if (parsed.render?.profile === 'vector-default' && parsed.render.band !== undefined) throw new Error('Invalid vector render fixture field.');
  if (parsed.license.verification === 'review-required' && parsed.license.termsUrl) throw new Error('Invalid review-required licence fixture field.');
  if (parsed.license.verification === 'confirmed' && !parsed.license.termsUrl) throw new Error('Invalid confirmed licence fixture field.');
  if (parsed.executable && !isExecutableFixture(parsed)) throw new Error('Invalid executable fixture state.');
  return parsed;
}

export function demoFixturesFromYaml(raw: string): DemoFixture[] {
  const root = record(parse(raw), 'inventory');
  if (root.version !== 1) throw new Error('Invalid inventory version.');
  if (!Array.isArray(root.examples)) throw new Error('The public demo inventory has no examples.');
  const fixtures = root.examples.map(fixture);
  if (new Set(fixtures.map((value) => value.id)).size !== fixtures.length) throw new Error('Invalid duplicate fixture id.');
  if (!fixtures.some(isExecutableFixture)) {
    throw new Error('Invalid executable fixture inventory.');
  }
  return fixtures;
}

export function isExecutableFixture(value: DemoFixture): boolean {
  const etag = value.content.expectedStrongEtag;
  const resource = value.resource;
  const common = value.id.length <= 128 &&
    value.status === 'active' && value.executable && value.connector.state === 'ready' &&
    value.activation.state === 'approved' && value.license.verification === 'confirmed' &&
    value.content.kind === 'object' &&
    eligibleDemoSourceUrl(value.url).ok &&
    typeof value.content.expectedLength === 'number' && Number.isInteger(value.content.expectedLength) && value.content.expectedLength > 0 &&
    typeof etag === 'string' && etag.length > 2 && !etag.startsWith('W/') && etag.startsWith('"') && etag.endsWith('"') &&
    !!resource && resource.crs === 'EPSG:4326' && !!resource.selected.trim();
  if (!common || !resource) return false;
  if (value.format === 'tiled-geotiff') {
    return value.transport === 'range-native' && !!value.render &&
      value.render.profile === 'categorical-land-cover' &&
      Number.isInteger(value.render.band) && (value.render.band ?? 0) > 0;
  }
  return vectorFormats.has(value.format) &&
    ((value.format === 'geoparquet' && value.transport === 'range-native') ||
      (value.format === 'shapefile-zip' && value.transport === 'bounded-spool')) &&
    value.render?.profile === 'vector-default' &&
    !!resource.geometryType &&
    Number.isInteger(resource.featureCount) && (resource.featureCount ?? -1) >= 0 &&
    !!resource.attributes?.length &&
    !!resource.testedInitialView;
}

export function safeDemoFixturesFromYaml(raw: string): { fixtures: DemoFixture[]; error: string | null } {
  try {
    const fixtures = demoFixturesFromYaml(raw);
    return { fixtures, error: null };
  } catch {
    return { fixtures: [], error: 'The verified example inventory is unavailable.' };
  }
}

const inventory = safeDemoFixturesFromYaml(publicExamplesYaml);
export const demoFixtures = inventory.fixtures;
export const demoFixtureInventoryError = inventory.error;

export function isScientificFixture(value: DemoFixture): boolean {
  return ['grib2', 'hdf5', 'netcdf', 'geozarr'].includes(value.format);
}

export function vectorControlsForFixture(value: DemoFixture): boolean {
  return ['geoparquet', 'shapefile-zip', 'geojson', 'flatgeobuf', 'pmtiles', 'geopackage'].includes(value.format);
}
