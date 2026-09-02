import { afterEach, describe, expect, it, vi } from 'vitest';
import { deleteDemoSource, registerDemoSource } from './demo-source-api';
import type { DemoSourceResponse } from './demo-source';

const wireResponse = {
  id: 'opaque', format: 'tiled-geotiff', transport: 'range-native', revision: 'strong',
  capability_state: 'ready', extent: null, attribution: 'Verified attribution',
  geometry_type: null, srid: null, number_matched: null, properties: [],
  limits: { expires_in_seconds: 900, max_live_sources: 3, max_concurrent_operations: 2 },
  links: { self_href: '/demo/sources/opaque', tile_template: '/demo/sources/opaque/tiles/WebMercatorQuad/{z}/{y}/{x}.png' },
};
const response: DemoSourceResponse = {
  id: 'opaque', format: 'tiled-geotiff', transport: 'range-native', revision: 'strong',
  capability_state: 'ready', extent: null, attribution: 'Verified attribution',
  geometryType: null, srid: null, numberMatched: null, properties: [],
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
