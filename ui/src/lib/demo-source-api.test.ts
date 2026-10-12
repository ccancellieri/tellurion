import { afterEach, describe, expect, it, vi } from 'vitest';
import { deleteDemoSource, registerDemoSource } from './demo-source-api';
import type { DemoSourceResponse } from './demo-source';

const wireResponse = {
  id: 'opaque', format: 'tiled-geotiff', transport: 'range-native', revision: 'strong',
  capability_state: 'ready', extent: null, attribution: 'Verified attribution',
  geometry_type: null, srid: null, number_matched: null, properties: [],
  raster: {
    layout: 'rgb', channel_count: 3, bits_per_sample: 8, epsg: 4326,
    shape: [256, 256], pixel_size: [0.01, 0.01],
    levels: [{ shape: [256, 256], tile_shape: [128, 128], pixel_size: [0.01, 0.01] }],
    nodata: { state: 'absent' }, profiles: [{ id: 'native-rgb', label: 'Native color' }],
    default_profile: 'native-rgb', legend: [],
  },
  limits: { expires_in_seconds: 900, max_live_sources: 3, max_concurrent_operations: 2 },
  links: {
    self_href: '/demo/sources/opaque',
    tile_template: '/demo/sources/opaque/tiles/WebMercatorQuad/{z}/{y}/{x}.png?profile=native-rgb',
    raster_profiles: [{ id: 'native-rgb', tile_template: '/demo/sources/opaque/tiles/WebMercatorQuad/{z}/{y}/{x}.png?profile=native-rgb' }],
  },
};
const response: DemoSourceResponse = {
  id: 'opaque', format: 'tiled-geotiff', transport: 'range-native', revision: 'strong',
  capability_state: 'ready', extent: null, attribution: 'Verified attribution',
  geometryType: null, srid: null, numberMatched: null, properties: [],
  raster: {
    layout: 'rgb', channelCount: 3, bitsPerSample: 8, epsg: 4326,
    shape: [256, 256], pixelSize: [0.01, 0.01],
    levels: [{ shape: [256, 256], tileShape: [128, 128], pixelSize: [0.01, 0.01] }],
    nodata: { state: 'absent' }, unit: null,
    profiles: [{ id: 'native-rgb', label: 'Native color' }], defaultProfile: 'native-rgb', legend: [],
  },
  limits: wireResponse.limits,
  links: wireResponse.links,
};

afterEach(() => vi.unstubAllGlobals());

describe('demo source API', () => {
  it('posts same-origin JSON with credentials and returns the opaque response', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(wireResponse), { status: 200 }),
    );
    vi.stubGlobal('fetch', fetchMock);

    await expect(registerDemoSource('https://example.org/source.tif')).resolves.toMatchObject({ id: 'opaque' });
    expect(fetchMock).toHaveBeenCalledWith('/demo/sources', {
      method: 'POST',
      credentials: 'include',
      headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
      body: JSON.stringify({ url: 'https://example.org/source.tif' }),
      signal: undefined,
    });
  });

  it('reports an opaque status without leaking a failed URL', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('', { status: 422 })));

    await expect(registerDemoSource('https://example.org/secret.tif')).rejects.toThrow(
      'Source inspection was not accepted (422).',
    );
  });

  it.each(['cog', 'geoparquet', 'shapefile-zip'] as const)('posts the exact optional %s format hint', async (format) => {
    const fetchMock = vi.fn().mockResolvedValue(
      new Response(JSON.stringify(wireResponse), { status: 200 }),
    );
    vi.stubGlobal('fetch', fetchMock);

    await registerDemoSource('https://example.org/source', undefined, format);
    expect(JSON.parse(fetchMock.mock.calls[0][1].body)).toEqual({
      url: 'https://example.org/source', format,
    });
  });

  it('deletes only an opaque server id with the session credentials', async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 204 }));
    vi.stubGlobal('fetch', fetchMock);

    await deleteDemoSource({ ...response, id: 'opaque-source', links: { self_href: '/demo/sources/opaque-source', tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png' } });

    expect(fetchMock).toHaveBeenCalledWith('http://localhost/demo/sources/opaque-source', {
      method: 'DELETE',
      credentials: 'include',
      headers: { Accept: 'application/json' },
    });
  });

  it('rejects mismatched response metadata before it becomes workflow state', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify({ ...wireResponse, links: { ...wireResponse.links, self_href: '/demo/sources/other' } }), { status: 200 })));
    await expect(registerDemoSource('https://example.org/source.tif')).rejects.toThrow(
      'Source inspection returned invalid metadata.',
    );
  });

  it('keeps a failed deletion retryable', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('', { status: 503 })));
    await expect(deleteDemoSource(response)).rejects.toThrow('Source inspection was not accepted (503).');
  });
});
