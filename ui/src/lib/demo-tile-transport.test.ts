import { afterEach, describe, expect, it, vi } from 'vitest';

type ProtocolHandler = (
  request: { url: string },
  controller: AbortController,
) => Promise<{ data: ArrayBuffer }>;

const maplibre = vi.hoisted(() => ({
  addProtocol: vi.fn((_name: string, _handler: ProtocolHandler) => undefined),
  removeProtocol: vi.fn((_name: string) => undefined),
}));

vi.mock('maplibre-gl', () => ({ default: maplibre }));

import { createDemoTileTransport } from './map';

const ORIGIN = 'https://tellurion.example';

function activate(transport: ReturnType<typeof createDemoTileTransport>, sourceId: string): string {
  const template = transport.activate(
    sourceId,
    ORIGIN,
    `${ORIGIN}/demo/sources/${sourceId}/tiles/WebMercatorQuad/{z}/{y}/{x}.png`,
    2,
  );
  expect(template).not.toBeNull();
  return template!;
}

function tile(template: string, column: number): string {
  return template.replace('{z}', '0').replace('{y}', '0').replace('{x}', String(column));
}

afterEach(() => {
  vi.unstubAllGlobals();
  maplibre.addProtocol.mockReset();
  maplibre.removeProtocol.mockReset();
});

describe('MapLibre public-demo tile transport', () => {
  it('keeps one dispatcher working until the last of two transports is destroyed', async () => {
    const fetchMock = vi.fn(async (_url: RequestInfo | URL, _init?: RequestInit) =>
      new Response(new Uint8Array([1]), { status: 200, headers: { 'Content-Type': 'image/png' } }));
    vi.stubGlobal('fetch', fetchMock);
    const first = createDemoTileTransport();
    const second = createDemoTileTransport();
    try {
      expect(maplibre.addProtocol).toHaveBeenCalledOnce();
      const handler = maplibre.addProtocol.mock.calls[0][1] as ProtocolHandler;
      const firstTemplate = activate(first, 'source-one');
      const secondTemplate = activate(second, 'source-two');

      await handler({ url: tile(firstTemplate, 0) }, new AbortController());
      await handler({ url: tile(secondTemplate, 1) }, new AbortController());
      expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
        `${ORIGIN}/demo/sources/source-one/tiles/WebMercatorQuad/0/0/0.png`,
        `${ORIGIN}/demo/sources/source-two/tiles/WebMercatorQuad/0/0/1.png`,
      ]);
      for (const [, init] of fetchMock.mock.calls) {
        expect(init).toEqual(expect.objectContaining({
          credentials: 'include',
          headers: { Accept: 'image/png' },
          signal: expect.any(AbortSignal),
        }));
      }

      first.destroy();
      expect(maplibre.removeProtocol).not.toHaveBeenCalled();
      await expect(handler({ url: tile(firstTemplate, 2) }, new AbortController()))
        .rejects.toThrow('outside an active demo source');
      await handler({ url: tile(secondTemplate, 2) }, new AbortController());
      expect(fetchMock).toHaveBeenCalledTimes(3);
    } finally {
      first.destroy();
      second.destroy();
    }
    expect(maplibre.removeProtocol).toHaveBeenCalledOnce();
  });

  it.each([
    'tellurion-demo://outside.example/map-1/1/demo/sources/source-one/tiles/WebMercatorQuad/0/0/0.png',
    'tellurion-demo:///map-1/1/demo/sources/source-one/tiles/WebMercatorQuad/0/0/0.png?extra=true',
    'tellurion-demo:///map-1/1/demo/sources/source-one/tiles/WebMercatorQuad/0/0/0.png#extra',
    'tellurion-demo:///invalid/1/demo/sources/source-one/tiles/WebMercatorQuad/0/0/0.png',
  ])('rejects an unowned protocol URL: %s', async (url) => {
    const transport = createDemoTileTransport();
    try {
      const handler = maplibre.addProtocol.mock.calls[0][1] as ProtocolHandler;
      await expect(handler({ url }, new AbortController()))
        .rejects.toThrow('outside an active demo source');
    } finally {
      transport.destroy();
    }
  });
});
