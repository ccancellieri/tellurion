export const MAX_VISIBLE_3D_TILES = 64;
export const MIN_AFFINE_3D_ZOOM = 14;

const WEB_MERCATOR_MAX_LATITUDE = 85.0511287798066;
const WEB_MERCATOR_ROOT_ERROR = 156543.03392804097;
const WGS84_SEMI_MAJOR_AXIS = 6_378_137;
const WGS84_ECCENTRICITY_SQUARED = 6.69437999014e-3;

type Bbox = [number, number, number, number];

interface TilesetRoot {
  boundingVolume?: { region?: unknown };
  content?: { uri?: unknown; url?: unknown };
  geometricError?: unknown;
  implicitTiling?: {
    availableLevels?: unknown;
    subdivisionScheme?: unknown;
  };
}

interface TilesetDocument {
  asset?: { version?: unknown };
  geometricError?: unknown;
  root?: TilesetRoot;
}

export interface ExplicitTile {
  boundingVolume: { region: [number, number, number, number, number, number] };
  content: { uri: string };
  geometricError: number;
  transform: number[];
}

export interface ExplicitViewportTileset {
  asset: { version: '1.1'; gltfUpAxis: 'Z' };
  geometricError: number;
  root: {
    boundingVolume: { region: [number, number, number, number, number, number] };
    children: ExplicitTile[];
    geometricError: number;
    refine: 'REPLACE';
  };
}

export type ViewportTilesetResult =
  | { kind: 'manifest'; maxTiles: number; tileCount: number; tileset: ExplicitViewportTileset; zoom: number }
  | { kind: 'too-many'; maxTiles: number; tileCount: number; zoom: number }
  | { kind: 'too-coarse'; maxTiles: number; minimumZoom: number; requestedZoom: number; tileCount: 0 }
  | { kind: 'empty'; maxTiles: number; tileCount: 0; zoom: number };

export interface ViewportTilesetInput {
  tileset: unknown;
  rootUrl: string;
  pageUrl: string;
  collectionExtent: Bbox;
  viewport: Bbox;
  requestedZoom: number;
  maxTiles?: number;
}

function fail(detail: string): never {
  throw new Error(`Invalid 3D tileset: ${detail}`);
}

function finiteNumber(value: unknown, name: string): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) fail(`${name} must be finite`);
  return value;
}

function validBbox(value: Bbox, name: string): Bbox {
  if (
    !Array.isArray(value)
    || value.length !== 4
    || value.some((coordinate) => !Number.isFinite(coordinate))
    || value[0] > value[2]
    || value[1] > value[3]
  ) {
    fail(`${name} is invalid`);
  }
  return value;
}

function intersectBboxes(a: Bbox, b: Bbox): Bbox | null {
  const intersection: Bbox = [
    Math.max(a[0], b[0]),
    Math.max(a[1], b[1]),
    Math.min(a[2], b[2]),
    Math.min(a[3], b[3]),
  ];
  return intersection[0] <= intersection[2] && intersection[1] <= intersection[3]
    ? intersection
    : null;
}

function clampLatitude(latitude: number): number {
  return Math.max(-WEB_MERCATOR_MAX_LATITUDE, Math.min(WEB_MERCATOR_MAX_LATITUDE, latitude));
}

function tileColumn(longitude: number, zoom: number): number {
  const side = 2 ** zoom;
  return Math.max(0, Math.min(side - 1, Math.floor(((longitude + 180) / 360) * side)));
}

function tileRow(latitude: number, zoom: number): number {
  const side = 2 ** zoom;
  const radians = clampLatitude(latitude) * Math.PI / 180;
  const projected = (1 - Math.asinh(Math.tan(radians)) / Math.PI) / 2;
  return Math.max(0, Math.min(side - 1, Math.floor(projected * side)));
}

function tileRegion(
  zoom: number,
  x: number,
  y: number,
  minimumHeight: number,
  maximumHeight: number,
): [number, number, number, number, number, number] {
  const side = 2 ** zoom;
  const west = (x / side * 2 - 1) * Math.PI;
  const east = ((x + 1) / side * 2 - 1) * Math.PI;
  const north = Math.atan(Math.sinh(Math.PI * (1 - 2 * y / side)));
  const south = Math.atan(Math.sinh(Math.PI * (1 - 2 * (y + 1) / side)));
  return [west, south, east, north, minimumHeight, maximumHeight];
}

function ecefTransform(zoom: number, x: number, y: number): number[] {
  const side = 2 ** zoom;
  const longitude = ((x + 0.5) / side * 2 - 1) * Math.PI;
  const latitude = Math.atan(Math.sinh(Math.PI * (1 - 2 * (y + 0.5) / side)));
  const sinLongitude = Math.sin(longitude);
  const cosLongitude = Math.cos(longitude);
  const sinLatitude = Math.sin(latitude);
  const cosLatitude = Math.cos(latitude);
  const primeVerticalRadius = WGS84_SEMI_MAJOR_AXIS
    / Math.sqrt(1 - WGS84_ECCENTRICITY_SQUARED * sinLatitude ** 2);
  const meridianRadius = WGS84_SEMI_MAJOR_AXIS * (1 - WGS84_ECCENTRICITY_SQUARED)
    / (1 - WGS84_ECCENTRICITY_SQUARED * sinLatitude ** 2) ** 1.5;
  const radiansPerTile = 2 * Math.PI / side;
  const xScale = primeVerticalRadius * cosLatitude * radiansPerTile;
  const yScale = meridianRadius * cosLatitude * radiansPerTile;

  const east = [-sinLongitude, cosLongitude, 0];
  const south = [
    sinLatitude * cosLongitude,
    sinLatitude * sinLongitude,
    -cosLatitude,
  ];
  const up = [
    cosLatitude * cosLongitude,
    cosLatitude * sinLongitude,
    sinLatitude,
  ];
  const center = [
    primeVerticalRadius * cosLatitude * cosLongitude,
    primeVerticalRadius * cosLatitude * sinLongitude,
    primeVerticalRadius * (1 - WGS84_ECCENTRICITY_SQUARED) * sinLatitude,
  ];
  const xAxis = east.map((component) => component * xScale);
  const yAxis = south.map((component) => component * yScale);
  const origin = center.map((component, index) => component - xAxis[index] / 2 - yAxis[index] / 2);

  return [
    xAxis[0], xAxis[1], xAxis[2], 0,
    yAxis[0], yAxis[1], yAxis[2], 0,
    up[0], up[1], up[2], 0,
    origin[0], origin[1], origin[2], 1,
  ];
}

function validateTemplate(value: unknown, rootUrl: URL): string {
  if (typeof value !== 'string') fail('root content URI is missing');
  const template = new URL(value, `${rootUrl.toString().replace(/\/$/, '')}/`);
  const templatePath = template.pathname.replace(/%7B/gi, '{').replace(/%7D/gi, '}');
  if (
    template.protocol !== rootUrl.protocol
    || template.origin !== rootUrl.origin
    || template.username
    || template.password
    || template.search
    || template.hash
    || templatePath !== `${rootUrl.pathname.replace(/\/$/, '')}/tiles/{level}/{y}/{x}.glb`
  ) {
    fail('root content URI must be its same-origin level/row/column GLB template');
  }
  return template.toString().replace(/%7B/gi, '{').replace(/%7D/gi, '}');
}

function deriveZoomRange(root: TilesetRoot, levels: number): [number, number] {
  const rootError = finiteNumber(root.geometricError, 'root geometricError');
  if (rootError <= 0) fail('root geometricError must be positive');
  const candidate = Math.log2(WEB_MERCATOR_ROOT_ERROR / rootError);
  const minimumZoom = Math.round(candidate);
  if (
    minimumZoom < 0
    || minimumZoom > 30
    || Math.abs(candidate - minimumZoom) > 1e-6
    || minimumZoom + levels - 1 > 30
  ) {
    fail('root geometricError does not identify a supported WebMercatorQuad range');
  }
  return [minimumZoom, minimumZoom + levels - 1];
}

/**
 * Replaces Tellurion's constant-availability implicit root with one finite,
 * flat tileset. Each child is a requested WebMercatorQuad tile whose local
 * X/Y/Z axes are placed in WGS84 ECEF before loaders.gl renders the GLB.
 */
export function buildViewportTileset(input: ViewportTilesetInput): ViewportTilesetResult {
  const document = input.tileset as TilesetDocument | null;
  if (!document || typeof document !== 'object' || !document.root || document.asset?.version !== '1.1') {
    fail('asset 1.1 and root are required');
  }
  const root = document.root;
  const implicit = root.implicitTiling;
  if (!implicit || implicit.subdivisionScheme !== 'QUADTREE') {
    fail('a QUADTREE implicit root is required');
  }
  const levels = finiteNumber(implicit.availableLevels, 'availableLevels');
  if (!Number.isSafeInteger(levels) || levels < 1) fail('availableLevels must be a positive integer');
  const rootRegion = root.boundingVolume?.region;
  if (
    !Array.isArray(rootRegion)
    || rootRegion.length !== 6
    || rootRegion.some((value) => typeof value !== 'number' || !Number.isFinite(value))
  ) {
    fail('root bounding region is invalid');
  }
  const minimumHeight = rootRegion[4] as number;
  const maximumHeight = rootRegion[5] as number;
  if (minimumHeight > maximumHeight) fail('root height range is invalid');

  const pageUrl = new URL(input.pageUrl);
  const rootUrl = new URL(input.rootUrl, pageUrl);
  if (!['http:', 'https:'].includes(rootUrl.protocol) || rootUrl.origin !== pageUrl.origin) {
    fail('root URL must use the page origin');
  }
  const template = validateTemplate(root.content?.uri ?? root.content?.url, rootUrl);
  const [minimumZoom, maximumZoom] = deriveZoomRange(root, levels);
  const requestedZoom = Math.floor(finiteNumber(input.requestedZoom, 'requested zoom'));
  const maxTiles = input.maxTiles ?? MAX_VISIBLE_3D_TILES;
  if (!Number.isSafeInteger(maxTiles) || maxTiles < 1) fail('tile budget must be a positive integer');
  const supportedMinimumZoom = Math.max(minimumZoom, MIN_AFFINE_3D_ZOOM);
  if (maximumZoom < supportedMinimumZoom) {
    fail('advertised zoom range is below the supported placement precision');
  }
  if (requestedZoom < supportedMinimumZoom) {
    return {
      kind: 'too-coarse',
      requestedZoom,
      minimumZoom: supportedMinimumZoom,
      tileCount: 0,
      maxTiles,
    };
  }
  const zoom = Math.min(maximumZoom, requestedZoom);

  const collectionExtent = validBbox(input.collectionExtent, 'collection extent');
  const viewport = validBbox(input.viewport, 'viewport');
  const intersection = intersectBboxes(collectionExtent, viewport);
  if (!intersection) return { kind: 'empty', zoom, tileCount: 0, maxTiles };

  const minimumColumn = tileColumn(intersection[0], zoom);
  const maximumColumn = tileColumn(intersection[2], zoom);
  const minimumRow = tileRow(intersection[3], zoom);
  const maximumRow = tileRow(intersection[1], zoom);
  const tileCount = (maximumColumn - minimumColumn + 1) * (maximumRow - minimumRow + 1);
  if (tileCount > maxTiles) return { kind: 'too-many', zoom, tileCount, maxTiles };

  const children: ExplicitTile[] = [];
  for (let y = minimumRow; y <= maximumRow; y += 1) {
    for (let x = minimumColumn; x <= maximumColumn; x += 1) {
      children.push({
        boundingVolume: { region: tileRegion(zoom, x, y, minimumHeight, maximumHeight) },
        content: {
          uri: template
            .replace('{level}', String(zoom))
            .replace('{y}', String(y))
            .replace('{x}', String(x)),
        },
        geometricError: 0,
        transform: ecefTransform(zoom, x, y),
      });
    }
  }

  const [west, south, east, north] = intersection;
  const geometricError = finiteNumber(document.geometricError ?? root.geometricError, 'geometricError');
  return {
    kind: 'manifest',
    maxTiles,
    tileCount,
    zoom,
    tileset: {
      asset: { version: '1.1', gltfUpAxis: 'Z' },
      geometricError,
      root: {
        boundingVolume: {
          region: [
            west * Math.PI / 180,
            south * Math.PI / 180,
            east * Math.PI / 180,
            north * Math.PI / 180,
            minimumHeight,
            maximumHeight,
          ],
        },
        children,
        geometricError,
        refine: 'REPLACE',
      },
    },
  };
}
