// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { parse } from '@loaders.gl/core';
import { Tiles3DLoader } from '@loaders.gl/3d-tiles';
import { Tileset3D } from '@loaders.gl/tiles';
import { createEmptyTileAdapter } from './places3d-empty-content';

const url = 'https://tellurion.example/tiles/15/11947/17058.glb';

describe('3D empty content adapter', () => {
  async function realTile(status: number) {
    const data = await parse(new TextEncoder().encode(JSON.stringify({
      asset: { version: '1.1' }, geometricError: 1,
      root: { boundingVolume: { region: [0, 0, 0.01, 0.01, 0, 10] },
        geometricError: 0, content: { uri: url } },
    })), Tiles3DLoader, { '3d-tiles': { isTileset: true } });
    // The Node loader resolves URLs as filesystem paths; use its parsed URL.
    const adapter = createEmptyTileAdapter([data.root.contentUrl], new AbortController().signal,
      async () => new Response(null, { status }));
    const tileset = new Tileset3D(data, {
      throttleRequests: false,
      loadOptions: { core: { fetch: adapter.fetch } },
      contentLoader: adapter.contentLoader,
    });
    if (!tileset.root) throw new Error('Test tileset root is missing');
    return tileset.root;
  }

  it('passes a real Tile3D load and scheduler with zero GPU memory for 204', async () => {
    const tile = await realTile(204);
    expect(await tile.loadContent()).toBe(true);
    expect(tile.hasEmptyContent).toBe(true);
    expect(tile.hasRenderContent).toBe(false);
    expect(tile.contentAvailable).toBe(false);
    expect(tile.gpuMemoryUsageInBytes).toBe(0);
  });

  it('does not hide a malformed 200 response from the real loader', async () => {
    const tile = await realTile(200);
    await expect(tile.loadContent()).rejects.toThrow();
    expect(tile.hasEmptyContent).toBe(false);
  });

  it('marks only a confirmed 204 tile empty without a second request', async () => {
    const fetcher = vi.fn(async () => new Response(null, { status: 204 }));
    const adapter = createEmptyTileAdapter([url], new AbortController().signal, fetcher);
    await adapter.fetch(url);
    const tile = { contentUrl: url, content: null, hasEmptyContent: false };
    await adapter.contentLoader(tile);
    expect(tile.hasEmptyContent).toBe(true);
    expect(tile.content).toEqual({ byteLength: 0, gpuMemoryUsageInBytes: 0 });
    expect(fetcher).toHaveBeenCalledTimes(1);
  });

  it('leaves null 200 and unrequested content errors visible', async () => {
    const adapter = createEmptyTileAdapter([url], new AbortController().signal,
      async () => new Response(null, { status: 200 }));
    await adapter.fetch(url);
    const tile = { contentUrl: url, content: null, hasEmptyContent: false };
    await adapter.contentLoader(tile);
    expect(tile).toEqual({ contentUrl: url, content: null, hasEmptyContent: false });
  });

  it('clears an earlier 204 marker when a retry returns malformed 200', async () => {
    let status = 204;
    const adapter = createEmptyTileAdapter([url], new AbortController().signal,
      async () => new Response(null, { status }));
    await adapter.fetch(url);
    status = 200;
    await adapter.fetch(url);
    const tile = { contentUrl: url, content: null, hasEmptyContent: false };
    await adapter.contentLoader(tile);
    expect(tile.hasEmptyContent).toBe(false);
  });

  it('does not turn an unrelated URL into empty tile content', async () => {
    const adapter = createEmptyTileAdapter([url], new AbortController().signal,
      async () => new Response(null, { status: 204 }));
    const other = 'https://tellurion.example/other.glb';
    await adapter.fetch(other);
    const tile = { contentUrl: other, content: null, hasEmptyContent: false };
    await adapter.contentLoader(tile);
    expect(tile.hasEmptyContent).toBe(false);
  });

  it('keeps the generation abort signal on tile requests', async () => {
    const controller = new AbortController();
    const fetcher = vi.fn(async () => new Response(null, { status: 204 }));
    const adapter = createEmptyTileAdapter([url], controller.signal, fetcher);
    await adapter.fetch(url);
    expect(fetcher).toHaveBeenCalledWith(url, { signal: controller.signal });
    controller.abort();
    const tile = { contentUrl: url, content: null, hasEmptyContent: false };
    await adapter.contentLoader(tile);
    expect(tile.hasEmptyContent).toBe(false);
  });
});
