/** @vitest-environment happy-dom */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const map = vi.hoisted(() => ({
  addLayer: vi.fn(),
  addSource: vi.fn(),
  fitBounds: vi.fn(),
  getLayer: vi.fn(),
  getSource: vi.fn(),
  isStyleLoaded: vi.fn(),
  on: vi.fn(),
  off: vi.fn(),
  queryRenderedFeatures: vi.fn(),
  getCanvas: vi.fn(),
  remove: vi.fn(),
  removeLayer: vi.fn(),
  removeSource: vi.fn(),
}));

const tileTransport = vi.hoisted(() => ({
  activate: vi.fn((sourceId: string) =>
    `tellurion-demo:///demo/sources/${sourceId}/tiles/WebMercatorQuad/{z}/{y}/{x}.png`),
  clear: vi.fn(),
  destroy: vi.fn(),
}));

vi.mock('../lib/map', () => ({
  createMap: vi.fn(() => map),
  createDemoTileTransport: vi.fn(() => tileTransport),
  demoRasterMapHandoff: vi.fn((source: { id: string; attribution: string; extent: unknown }) => ({
    sourceId: `demo-source-${source.id}`,
    layerId: `demo-layer-${source.id}`,
    template: `https://tellurion.example/demo/sources/${source.id}/tiles/WebMercatorQuad/{z}/{y}/{x}.png`,
    extent: source.extent,
    attribution: source.attribution,
  })),
  demoVectorMapHandoff: vi.fn((source: { id: string; attribution: string; extent: [number, number, number, number]; geometryType: string }) => ({
    sourceId: `demo-source-${source.id}`,
    layerId: `demo-layer-${source.id}`,
    sourceLayer: source.id,
    template: `https://tellurion.example/demo/sources/${source.id}/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt`,
    extent: source.extent,
    attribution: source.attribution,
    geometryType: source.geometryType,
  })),
  fitToExtent: vi.fn(),
}));

import './demo-map-viewer';
import { createDemoTileTransport, demoRasterMapHandoff, fitToExtent } from '../lib/map';

const source = {
  id: 'opaque-source',
  format: 'tiled-geotiff',
  transport: 'range-native',
  revision: 'strong',
  capability_state: 'ready',
  extent: [12, 39, 15, 42] as [number, number, number, number],
  geometryType: null,
  srid: null,
  numberMatched: null,
  properties: [],
  attribution: 'Verified attribution',
  limits: { expires_in_seconds: 900, max_live_sources: 3, max_concurrent_operations: 2 },
  links: {
    self_href: '/demo/sources/opaque-source',
    tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
  },
};

function mount(): HTMLElement {
  const element = document.createElement('tellurion-demo-map-viewer');
  document.body.append(element);
  return element;
}

beforeEach(() => {
  Object.values(map).forEach((mock) => mock.mockReset());
  Object.values(tileTransport).forEach((mock) => mock.mockClear());
  vi.mocked(fitToExtent).mockReset();
  vi.mocked(createDemoTileTransport).mockClear();
  map.isStyleLoaded.mockReturnValue(true);
  map.getLayer.mockReturnValue(undefined);
  map.getSource.mockReturnValue(undefined);
  map.queryRenderedFeatures.mockReturnValue([]);
  map.getCanvas.mockReturnValue({ clientWidth: 800, clientHeight: 400 });
});

describe('temporary map tile errors', () => {
  function open(value: unknown = source): void {
    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source: value, opacity: 0.7 } }));
  }
  function emit(event: string, detail: unknown): void {
    map.on.mock.calls.filter(([name]) => name === event).forEach(([, listener]) => listener(detail));
  }
  function status(element: HTMLElement): string | null | undefined {
    return element.querySelector('[data-field="status"]')?.textContent;
  }

  it.each(['tiled-geotiff', 'geoparquet'])('warns about active %s tiles without exposing error details or changing the layer', (format) => {
    const element = mount();
    open({ ...source, format, geometryType: 'Polygon', srid: 4326, numberMatched: 7,
      links: { ...source.links, mvt_tile_template: '/demo/vector.mvt' } });
    map.addSource.mockClear();
    map.addLayer.mockClear();
    tileTransport.activate.mockClear();
    emit('error', { sourceId: 'demo-source-opaque-source', error: new Error('https://private.example/?secret=token <img src=x>') });
    expect(status(element)).toContain('Some map tiles could not be loaded');
    expect(status(element)).toContain('Try another zoom level');
    expect(element.textContent).not.toContain('private.example');
    expect(element.textContent).not.toContain('secret=token');
    expect(element.querySelector('img')).toBeNull();
    expect(map.addSource).not.toHaveBeenCalled();
    expect(map.addLayer).not.toHaveBeenCalled();
    expect(map.removeLayer).not.toHaveBeenCalled();
    expect(tileTransport.activate).not.toHaveBeenCalled();
    emit('sourcedata', { sourceId: 'demo-source-opaque-source', isSourceLoaded: true });
    expect(status(element)).toContain('Some map tiles could not be loaded');
  });

  it('ignores unscoped, unrelated and stale source errors', () => {
    const element = mount();
    open();
    const opened = status(element);
    for (const sourceId of [undefined, null, 123, 'other-source']) emit('error', { sourceId, error: new Error('unrelated') });
    expect(status(element)).toBe(opened);
    open({ ...source, id: 'replacement' });
    emit('error', { sourceId: 'demo-source-opaque-source', error: new Error('stale') });
    expect(status(element)).toBe(opened);
    emit('error', { sourceId: 'demo-source-replacement', error: new Error('active') });
    expect(status(element)).toContain('Some map tiles could not be loaded');
  });

  it.each(['reset', 'replacement', 'expiry'])('clears the error on %s and retains the original expiry deadline', (action) => {
    vi.useFakeTimers();
    try {
      const element = mount();
      open({ ...source, limits: { ...source.limits, expires_in_seconds: 2 } });
      vi.advanceTimersByTime(1000);
      emit('error', { sourceId: 'demo-source-opaque-source', error: new Error('failed tile') });
      expect(status(element)).toContain('Some map tiles could not be loaded');
      if (action === 'reset') document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', { detail: { sourceId: source.id } }));
      else if (action === 'replacement') open({ ...source, id: 'replacement' });
      else vi.advanceTimersByTime(1000);
      const next = status(element);
      expect(next).not.toContain('Some map tiles could not be loaded');
      if (action === 'expiry') expect(next).toContain('expired');
      emit('error', { sourceId: 'demo-source-opaque-source', error: new Error('late tile') });
      expect(status(element)).toBe(next);
    } finally { vi.useRealTimers(); }
  });

  it('removes its error listener on disconnect', () => {
    const element = mount();
    open();
    const listener = map.on.mock.calls.find(([name]) => name === 'error')?.[1];
    expect(listener).toBeTypeOf('function');
    element.remove();
    expect(map.off).toHaveBeenCalledWith('error', listener);
    const previous = status(element);
    listener?.({ sourceId: 'demo-source-opaque-source', error: new Error('late') });
    expect(status(element)).toBe(previous);
  });
});

describe('fit active source extent', () => {
  function open(value: unknown = source): void {
    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source: value, opacity: 0.7 } }));
  }
  function button(element: HTMLElement): HTMLButtonElement | null {
    return element.querySelector('[data-field="fit-extent"]');
  }

  it.each(['tiled-geotiff', 'geoparquet', 'shapefile-zip'])('refits %s without reopening the source', (format) => {
    const element = mount();
    expect(button(element)?.hidden).toBe(true);
    open({ ...source, format, geometryType: 'Polygon', srid: 4326, numberMatched: 7,
      links: { ...source.links, mvt_tile_template: '/demo/vector.mvt' } });
    expect(button(element)?.tagName).toBe('BUTTON');
    expect(button(element)?.textContent).toBe('Fit source extent');
    expect(button(element)?.hidden).toBe(false);
    vi.mocked(fitToExtent).mockClear();
    map.addSource.mockClear();
    map.addLayer.mockClear();
    tileTransport.activate.mockClear();
    button(element)?.click();
    expect(fitToExtent).toHaveBeenCalledWith(map, { spatial: { bbox: [[12, 39, 15, 42]], crs: 'EPSG:4326' } });
    expect(map.addSource).not.toHaveBeenCalled();
    expect(map.addLayer).not.toHaveBeenCalled();
    expect(tileTransport.activate).not.toHaveBeenCalled();
  });

  it('uses validated active handoff bounds rather than source metadata or a pending source', () => {
    const element = mount();
    vi.mocked(demoRasterMapHandoff).mockReturnValueOnce({ sourceId: 'demo-source-opaque-source', layerId: 'demo-layer-opaque-source',
      template: '/demo/tiles.png', attribution: 'Verified attribution', extent: [1, 2, 3, 4] });
    open();
    map.isStyleLoaded.mockReturnValue(false);
    open({ ...source, id: 'pending', extent: [20, 30, 40, 50] });
    vi.mocked(fitToExtent).mockClear();
    button(element)?.click();
    expect(fitToExtent).toHaveBeenCalledWith(map, { spatial: { bbox: [[1, 2, 3, 4]], crs: 'EPSG:4326' } });
  });

  it('uses replacement bounds, and hides the action for a replacement without bounds', () => {
    const element = mount();
    open();
    open({ ...source, id: 'next', extent: [1, 2, 3, 4] });
    vi.mocked(fitToExtent).mockClear();
    button(element)?.click();
    expect(fitToExtent).toHaveBeenCalledWith(map, { spatial: { bbox: [[1, 2, 3, 4]], crs: 'EPSG:4326' } });
    open({ ...source, extent: null });
    expect(button(element)?.hidden).toBe(true);
    vi.mocked(fitToExtent).mockClear();
    button(element)?.click();
    expect(fitToExtent).not.toHaveBeenCalled();
  });

  it.each(['reset', 'expiry', 'disconnect'])('clears the action after %s without extending expiry when fitting', (action) => {
    vi.useFakeTimers();
    try {
      const element = mount();
      open({ ...source, limits: { ...source.limits, expires_in_seconds: 2 } });
      vi.advanceTimersByTime(1000);
      button(element)?.click();
      if (action === 'reset') document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', { detail: { sourceId: source.id } }));
      else if (action === 'expiry') vi.advanceTimersByTime(1000);
      else element.remove();
      expect(button(element)?.hidden).toBe(true);
      vi.mocked(fitToExtent).mockClear();
      button(element)?.click();
      expect(fitToExtent).not.toHaveBeenCalled();
    } finally { vi.useRealTimers(); }
  });

  it('retains the active extent on an unrelated reset', () => {
    const element = mount();
    open();
    document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', { detail: { sourceId: 'unrelated' } }));
    expect(button(element)?.hidden).toBe(false);
    vi.mocked(fitToExtent).mockClear();
    button(element)?.click();
    expect(fitToExtent).toHaveBeenCalledOnce();
  });
});

describe('rendered vector feature inspection', () => {
  const vector = { ...source, format: 'geoparquet', geometryType: 'Polygon', srid: 4326,
    numberMatched: 7, links: { ...source.links, mvt_tile_template: '/demo/vector.mvt' } };
  function open(elementSource = vector): void {
    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source: elementSource, opacity: 1 } }));
  }
  function click(): void {
    const listener = map.on.mock.calls.find(([event]) => event === 'click')?.[1];
    listener?.({ point: { x: 25, y: 30 } });
  }
  function field(element: HTMLElement, name: string): HTMLElement | null {
    return element.querySelector(`[data-field="${name}"]`);
  }

  it.each(['geoparquet', 'shapefile-zip'])('shows %s tile properties and id without interpreting HTML', (format) => {
    const element = mount();
    open({ ...vector, format });
    map.queryRenderedFeatures.mockReturnValue([{ id: 0, properties: { name: '<img src=x onerror=alert(1)>', count: 3, active: false, missing: null } }]);
    click();
    expect(field(element, 'feature-details')?.hidden).toBe(false);
    expect(field(element, 'feature-id')?.textContent).toBe('0');
    expect(field(element, 'feature-properties')?.textContent).toContain('<img src=x onerror=alert(1)>');
    expect(field(element, 'feature-properties')?.textContent).toContain('false');
    expect(field(element, 'feature-properties')?.textContent).toContain('null');
    expect(element.querySelector('img')).toBeNull();
    expect(map.queryRenderedFeatures).toHaveBeenCalledWith([[21, 26], [29, 34]], { layers: ['demo-layer-opaque-source'] });
    expect(element.textContent).toContain('simplified');
  });

  it('offers center inspection as a native keyboard-accessible button and closes details', () => {
    const element = mount();
    open();
    map.queryRenderedFeatures.mockReturnValue([{ properties: { name: 'Center feature' } }]);
    const button = field(element, 'inspect-center');
    expect(button?.tagName).toBe('BUTTON');
    button?.click();
    expect(map.queryRenderedFeatures).toHaveBeenCalledWith([[396, 196], [404, 204]], { layers: ['demo-layer-opaque-source'] });
    expect(field(element, 'feature-properties')?.textContent).toContain('Center feature');
    expect(field(element, 'feature-id')?.textContent).toContain('Not available');
    field(element, 'close-details')?.click();
    expect(field(element, 'feature-details')?.hidden).toBe(true);
  });

  it('uses a scalar id attribute when the tile has no top-level feature id', () => {
    const element = mount();
    open();
    map.queryRenderedFeatures.mockReturnValue([{ properties: { id: 'boundary-42' } }]);
    click();
    expect(field(element, 'feature-id')?.textContent).toBe('boundary-42');
    expect(field(element, 'feature-properties')?.textContent).toContain('boundary-42');
  });

  it('warns that integer IDs and attributes beyond JavaScript safe precision are approximate', () => {
    const element = mount();
    open();
    map.queryRenderedFeatures.mockReturnValue([{ id: 9007199254740992, properties: { s2_id: 9007199254740992, count: 42 } }]);
    click();
    expect(field(element, 'feature-id')?.textContent).toContain('approximate');
    const values = field(element, 'feature-properties')?.querySelectorAll('dd');
    expect(values?.[0]?.textContent).toContain('approximate');
    expect(values?.[0]?.textContent).toContain('safe integer precision');
    expect(values?.[1]?.textContent).toBe('42');
  });

  it('ignores raster clicks and hides inspection controls', () => {
    const element = mount();
    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source, opacity: 1 } }));
    click();
    expect(map.queryRenderedFeatures).not.toHaveBeenCalled();
    expect(field(element, 'inspect-controls')?.hidden).toBe(true);
  });

  it('clears old details and explains an empty selection', () => {
    const element = mount();
    open();
    map.queryRenderedFeatures.mockReturnValueOnce([{ id: 7, properties: { name: 'Old value' } }]);
    click();
    click();
    expect(field(element, 'feature-details')?.hidden).toBe(true);
    expect(field(element, 'feature-properties')?.textContent).not.toContain('Old value');
    expect(field(element, 'inspect-status')?.textContent).toContain('No rendered feature');
  });

  it.each(['reset', 'expiry', 'replacement'])('clears stale selection after %s', (action) => {
    vi.useFakeTimers();
    try {
      const element = mount();
      open({ ...vector, limits: { ...source.limits, expires_in_seconds: 2 } });
      map.queryRenderedFeatures.mockReturnValue([{ id: 7, properties: { name: 'Old value' } }]);
      click();
      if (action === 'reset') document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', { detail: { sourceId: source.id } }));
      else if (action === 'expiry') vi.advanceTimersByTime(2000);
      else open({ ...vector, id: 'replacement' });
      expect(field(element, 'feature-details')?.hidden).toBe(true);
      expect(field(element, 'feature-properties')?.textContent).not.toContain('Old value');
      if (action !== 'replacement') {
        map.queryRenderedFeatures.mockClear();
        click();
        expect(map.queryRenderedFeatures).not.toHaveBeenCalled();
      }
    } finally { vi.useRealTimers(); }
  });

  it('bounds properties, names and values and discloses truncation', () => {
    const element = mount();
    open();
    const properties = Object.fromEntries(Array.from({ length: 70 }, (_, index) => [`property-${index}`, 'x'.repeat(2000)]));
    map.queryRenderedFeatures.mockReturnValue([{ id: 'i'.repeat(2000), properties }]);
    click();
    const values = field(element, 'feature-properties')?.querySelectorAll('dd');
    expect(values?.length).toBe(64);
    expect(values?.[0]?.textContent?.length).toBeLessThanOrEqual(1025);
    expect(field(element, 'feature-id')?.textContent?.length).toBeLessThanOrEqual(1025);
    expect(field(element, 'inspect-status')?.textContent).toContain('truncated');
  });

  it('detaches its click listener on disconnect', () => {
    const element = mount();
    const listener = map.on.mock.calls.find(([event]) => event === 'click')?.[1];
    element.remove();
    expect(map.off).toHaveBeenCalledWith('click', listener);
  });
});

afterEach(() => {
  document.querySelectorAll('tellurion-demo-map-viewer').forEach((element) => element.remove());
  document.body.replaceChildren();
});

describe('temporary demo map viewer', () => {
  it('guides an empty session and restores the guidance after removing its source', () => {
    const element = mount();
    const empty = element.querySelector<HTMLElement>('[data-field="empty"]')!;
    expect(empty.hidden).toBe(false);
    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source, opacity: 1 } }));
    expect(empty.hidden).toBe(true);
    document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', { detail: { sourceId: source.id } }));
    expect(empty.hidden).toBe(false);
  });

  it('maps the same-origin handoff and reports its attribution', () => {
    const element = mount();

    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source, opacity: 0.7 } }));

    expect(map.addSource).toHaveBeenCalledWith('demo-source-opaque-source', {
      type: 'raster',
      tiles: ['tellurion-demo:///demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png'],
      tileSize: 256,
      minzoom: 0,
      maxzoom: 22,
    });
    expect(tileTransport.activate).toHaveBeenCalledWith(
      'opaque-source',
      location.origin,
      'https://tellurion.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
      2,
    );
    expect(tileTransport.activate.mock.invocationCallOrder[0]).toBeLessThan(
      map.addSource.mock.invocationCallOrder[0],
    );
    expect(map.addLayer).toHaveBeenCalledWith({
      id: 'demo-layer-opaque-source',
      type: 'raster',
      source: 'demo-source-opaque-source',
      paint: { 'raster-opacity': 0.7 },
    });
    expect(fitToExtent).toHaveBeenCalledWith(map, { spatial: { bbox: [[12, 39, 15, 42]], crs: 'EPSG:4326' } });
    expect(element.textContent).toContain('Verified attribution');
    expect(element.textContent).toContain('Temporary source map opened');
  });

  it('maps a valid source whose upstream metadata has no geographic extent', () => {
    mount();

    document.dispatchEvent(new CustomEvent('tellurion-demo-map', {
      detail: { source: { ...source, extent: null }, opacity: 1 },
    }));

    expect(map.addSource).toHaveBeenCalledOnce();
    expect(fitToExtent).not.toHaveBeenCalled();
  });

  it('removes only the requested temporary source and restores the empty state', () => {
    const element = mount();
    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source, opacity: 1 } }));
    map.getLayer.mockReturnValue({});
    map.getSource.mockReturnValue({});

    document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', { detail: { sourceId: source.id } }));

    expect(map.removeLayer).toHaveBeenCalledWith('demo-layer-opaque-source');
    expect(map.removeSource).toHaveBeenCalledWith('demo-source-opaque-source');
    expect(element.textContent).toContain('Choose a public HTTPS source');
  });

  it('does not cancel the active expiry timer for an unrelated reset event', () => {
    vi.useFakeTimers();
    try {
      mount();
      document.dispatchEvent(new CustomEvent('tellurion-demo-map', {
        detail: { source: { ...source, limits: { ...source.limits, expires_in_seconds: 2 } }, opacity: 1 },
      }));
      map.getLayer.mockReturnValue({});
      map.getSource.mockReturnValue({});

      document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', {
        detail: { sourceId: 'another-source' },
      }));
      vi.advanceTimersByTime(2_000);

      expect(map.removeLayer).toHaveBeenCalledWith('demo-layer-opaque-source');
      expect(map.removeSource).toHaveBeenCalledWith('demo-source-opaque-source');
    } finally {
      vi.useRealTimers();
    }
  });

  it('releases its map and document listeners when disconnected', () => {
    const element = mount();
    element.remove();

    expect(map.remove).toHaveBeenCalledOnce();
    expect(tileTransport.destroy).toHaveBeenCalledOnce();
    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source, opacity: 1 } }));
    expect(map.addSource).not.toHaveBeenCalled();
  });

  it('uses the advertised MVT template, opaque source layer, selected style, and extent for a vector source', () => {
    mount();
    const vector = {
      ...source,
      format: 'geoparquet',
      geometryType: 'Polygon',
      srid: 4326,
      numberMatched: 7,
      properties: ['boundary_id'],
      links: {
        ...source.links,
        items_href: '/demo/sources/opaque-source/items',
        item_template: '/demo/sources/opaque-source/items/{featureId}',
        mvt_tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt',
      },
    };

    document.dispatchEvent(new CustomEvent('tellurion-demo-map', {
      detail: { source: vector, opacity: 0.8, style: 'survey-ink' },
    }));

    expect(map.addSource).toHaveBeenCalledWith('demo-source-opaque-source', {
      type: 'vector',
      tiles: ['https://tellurion.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt'],
      minzoom: 0,
      maxzoom: 22,
    });
    expect(map.addLayer).toHaveBeenCalledWith(expect.objectContaining({
      type: 'fill', source: 'demo-source-opaque-source', 'source-layer': 'opaque-source',
      paint: expect.objectContaining({ 'fill-color': '#2e6970', 'fill-opacity': 0.8 }),
    }));
    expect(fitToExtent).toHaveBeenCalledWith(map, { spatial: { bbox: [[12, 39, 15, 42]], crs: 'EPSG:4326' } });
  });

  it('removes the temporary layer when the server-reported lifetime expires', () => {
    vi.useFakeTimers();
    try {
      const element = mount();
      const expired = vi.fn();
      document.addEventListener('tellurion-demo-source-expired', expired, { once: true });
      document.dispatchEvent(new CustomEvent('tellurion-demo-map', {
        detail: { source: { ...source, limits: { ...source.limits, expires_in_seconds: 2 } }, opacity: 1 },
      }));
      map.getLayer.mockReturnValue({});
      map.getSource.mockReturnValue({});

      vi.advanceTimersByTime(2_000);

      expect(map.removeLayer).toHaveBeenCalledWith('demo-layer-opaque-source');
      expect(map.removeSource).toHaveBeenCalledWith('demo-source-opaque-source');
      expect(tileTransport.clear).toHaveBeenCalled();
      expect(element.textContent).toContain('Temporary source expired');
      expect(expired).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });
});
