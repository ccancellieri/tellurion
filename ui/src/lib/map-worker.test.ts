// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from 'vitest';

const maplibre = vi.hoisted(() => {
  const events: string[] = [];
  return {
    events,
    setWorkerUrl: vi.fn((url: string) => events.push(`worker:${url}`)),
    Map: vi.fn(function Map() {
      events.push('map');
      return {};
    }),
    addProtocol: vi.fn(),
    removeProtocol: vi.fn(),
  };
});

vi.mock('maplibre-gl', () => maplibre);
vi.mock('maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url', () => ({
  default: '/assets/maplibre-gl-worker-test.js',
}));

describe('MapLibre worker setup', () => {
  beforeEach(() => {
    vi.resetModules();
    maplibre.events.length = 0;
    maplibre.setWorkerUrl.mockClear();
    maplibre.Map.mockClear();
  });

  it('configures the bundled local worker once before creating maps', async () => {
    const { createMap } = await import('./map');
    const container = document.createElement('div');

    createMap(container);
    createMap(container);

    expect(maplibre.events).toEqual([
      'worker:/assets/maplibre-gl-worker-test.js',
      'map',
      'map',
    ]);
    expect(maplibre.setWorkerUrl).toHaveBeenCalledOnce();
  });
});
