import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  fetchCollections,
  fetchDefaultCollections,
  fetchFeaturesLanding,
  fetchTenantDirectory,
  fetchTileSet,
  fetchTileSetList,
} from './api';

afterEach(() => {
  vi.unstubAllGlobals();
});

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

describe('typed API reads', () => {
  it('encodes an operator-supplied tenant directory path', async () => {
    const fetchMock = vi.fn().mockResolvedValue(
      jsonResponse({
        tenant: 'city team',
        links: [{ href: '/city%20team/features/catalogs/base', rel: 'features' }],
      }),
    );
    vi.stubGlobal('fetch', fetchMock);

    const directory = await fetchTenantDirectory('city team');

    expect(directory.tenant).toBe('city team');
    expect(fetchMock).toHaveBeenCalledWith('/city%20team', {
      headers: { Accept: 'application/json' },
      signal: undefined,
    });
  });

  it('follows advertised landing and collection URLs exactly', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(
        jsonResponse({ links: [{ href: '/city/features/catalogs/base/collections', rel: 'data' }] }),
      )
      .mockResolvedValueOnce(jsonResponse({ collections: [], links: [] }));
    vi.stubGlobal('fetch', fetchMock);

    await fetchFeaturesLanding('https://tellurion.example/city/features/catalogs/base');
    await fetchCollections(
      'https://tellurion.example/city/features/catalogs/base/collections?cursor=opaque',
    );

    expect(fetchMock.mock.calls.map(([href]) => href)).toEqual([
      'https://tellurion.example/city/features/catalogs/base',
      'https://tellurion.example/city/features/catalogs/base/collections?cursor=opaque',
    ]);
  });

  it('keeps the advanced protocol lab on its explicit default catalog', async () => {
    const fetchMock = vi.fn().mockResolvedValue(jsonResponse({ collections: [], links: [] }));
    vi.stubGlobal('fetch', fetchMock);

    await fetchDefaultCollections();

    expect(fetchMock.mock.calls[0][0]).toBe(
      '/public/features/catalogs/default/collections',
    );
  });

  it('reads the TileSet list and selected TileSet metadata without rebuilding paths', async () => {
    const listHref =
      'https://tellurion.example/city/tiles/catalogs/base/collections/roads/tiles';
    const tileSetHref = `${listHref}/WebMercatorQuad`;
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ tilesets: [{ links: [{ href: tileSetHref, rel: 'self' }] }] }))
      .mockResolvedValueOnce(
        jsonResponse({
          tileMatrixSetId: 'WebMercatorQuad',
          tileMatrixSetLimits: [{ tileMatrix: '0' }],
          mediaTypes: ['application/vnd.mapbox-vector-tile'],
          layers: [{ id: 'roads' }],
          links: [],
        }),
      );
    vi.stubGlobal('fetch', fetchMock);

    expect((await fetchTileSetList(listHref)).tilesets).toHaveLength(1);
    expect((await fetchTileSet(tileSetHref)).layers[0].id).toBe('roads');
    expect(fetchMock.mock.calls.map(([href]) => href)).toEqual([listHref, tileSetHref]);
  });

  it('uses a stable resource label instead of echoing a failed URL query', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(new Response('', { status: 503, statusText: 'Unavailable' })),
    );

    await expect(
      fetchCollections('https://tellurion.example/collections?cursor=secret-value'),
    ).rejects.toThrow('Collections request failed: 503 Unavailable');
  });
});
