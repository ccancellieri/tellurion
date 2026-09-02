/** @vitest-environment happy-dom */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DemoSourceResponse } from '../lib/demo-source';

const api = vi.hoisted(() => ({
  register: vi.fn(),
  remove: vi.fn(),
}));
const fixtures = vi.hoisted(() => [] as Array<Record<string, unknown>>);

vi.mock('../lib/demo-source-api', () => ({
  registerDemoSource: api.register,
  deleteDemoSource: api.remove,
  DemoSourceApiError: class DemoSourceApiError extends Error {
    status: number;
    constructor(status: number) {
      super(`status ${status}`);
      this.status = status;
    }
  },
}));

vi.mock('../lib/demo-fixtures', () => ({
  demoFixtures: fixtures,
  demoFixtureInventoryError: null,
  isExecutableFixture: (fixture: { executable?: boolean }) => fixture.executable === true,
  isScientificFixture: () => false,
  vectorControlsForFixture: (fixture: { format?: string }) => fixture.format === 'geoparquet' || fixture.format === 'shapefile-zip',
}));

import { workflowProgress } from './demo-source-workflow';

const source: DemoSourceResponse = {
  id: 'opaque-source',
  format: 'tiled-geotiff',
  transport: 'range-native',
  revision: 'strong',
  capability_state: 'ready',
  extent: [12, 39, 15, 42],
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

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((accept, decline) => {
    resolve = accept;
    reject = decline;
  });
  return { promise, resolve, reject };
}

function mount(): HTMLElement {
  const element = document.createElement('tellurion-demo-source-workflow');
  document.body.append(element);
  return element;
}

function submit(element: HTMLElement): void {
  const input = element.querySelector<HTMLInputElement>('#demo-source-url')!;
  input.value = 'https://data.example.org/worldcover.tif';
  element.querySelector<HTMLFormElement>('form')!.dispatchEvent(
    new Event('submit', { bubbles: true, cancelable: true }),
  );
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  api.register.mockReset();
  api.remove.mockReset();
  fixtures.length = 0;
});

afterEach(() => {
  document.body.replaceChildren();
});

describe('demo source workflow presentation', () => {
  it('marks the four truthful stages without calling publication a server mutation', () => {
    expect(workflowProgress('choose')).toEqual([
      { label: 'Choose', current: true, complete: false },
      { label: 'Inspect', current: false, complete: false },
      { label: 'Configure', current: false, complete: false },
      { label: 'Map', current: false, complete: false },
    ]);
    expect(workflowProgress('map')[3]).toEqual({ label: 'Map', current: true, complete: false });
  });

  it('keeps trusted curated attribution while discarding the source URL', async () => {
    fixtures.push({
      id: 'verified-vector', executable: true, title: 'Verified vector', provider: 'Publisher',
      attribution: 'Publisher dataset attribution', sourcePage: 'https://example.org/source',
      url: 'https://data.example.org/example.parquet', transport: 'range-native', format: 'geoparquet',
      license: { verification: 'confirmed', label: 'ODbL 1.0', termsUrl: 'https://example.org/terms' },
      connector: { state: 'ready', reason: 'Ready' }, content: { expectedLength: 42, expectedStrongEtag: '"tag"' },
      resource: { crs: 'EPSG:4326', selected: 'features' },
    });
    api.register.mockResolvedValue({ ...source, attribution: 'Remote source supplied by this browser session' });
    const element = mount();

    element.querySelector<HTMLButtonElement>('[data-example-id="verified-vector"]')!.click();
    await settle();

    expect(element.textContent).toContain('Publisher dataset attribution');
    expect(element.innerHTML).not.toContain('data.example.org');
  });

  it('returns to source selection when the mapped temporary source expires', async () => {
    api.register.mockResolvedValue(source);
    const element = mount();
    submit(element);
    await settle();
    element.querySelector<HTMLButtonElement>('[data-action="configure"]')!.click();
    element.querySelector<HTMLButtonElement>('[data-action="map"]')!.click();

    document.dispatchEvent(new CustomEvent('tellurion-demo-source-expired', {
      detail: { sourceId: source.id },
    }));

    expect(element.querySelector('#demo-source-url')).not.toBeNull();
    expect(element.textContent).toContain('Three temporary sources per browser session');
    expect(api.remove).not.toHaveBeenCalled();
  });

  it.each([
    ['geoparquet', 'range-native', 'https://data.example.org/buildings.parquet', 'Polygon'],
    ['shapefile-zip', 'bounded-zip-spool', 'https://data.example.org/coastline.zip', 'LineString'],
  ] as const)('runs the verified %s example through style, map, and removal', async (format, transport, url, geometryType) => {
    fixtures.push({
      id: `verified-${format}`, executable: true, title: `Verified ${format}`, provider: 'Publisher',
      attribution: 'Publisher dataset attribution', sourcePage: 'https://example.org/source',
      url, transport, format,
      license: { verification: 'confirmed', label: 'Verified terms', termsUrl: 'https://example.org/terms' },
      connector: { state: 'ready', reason: 'Ready' }, content: { expectedLength: 42, expectedStrongEtag: '"tag"' },
      resource: { crs: 'EPSG:4326', selected: 'features' },
    });
    const vectorSource: DemoSourceResponse = {
      ...source,
      format,
      transport,
      geometryType,
      srid: 4326,
      numberMatched: 7,
      properties: ['name', 'confidence'],
      links: {
        ...source.links,
        items_href: '/demo/sources/opaque-source/items',
        item_template: '/demo/sources/opaque-source/items/{featureId}',
        mvt_tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt',
      },
    };
    api.register.mockResolvedValue(vectorSource);
    api.remove.mockResolvedValue(undefined);
    const mapped = vi.fn();
    const element = mount();
    element.addEventListener('tellurion-demo-map', mapped, { once: true });

    element.querySelector<HTMLButtonElement>(`[data-example-id="verified-${format}"]`)!.click();
    await settle();
    expect(api.register).toHaveBeenCalledWith(url, expect.any(AbortSignal));
    expect(element.textContent).toContain('Fieldsname, confidence');

    element.querySelector<HTMLButtonElement>('[data-action="configure"]')!.click();
    expect(element.textContent).toContain('Vector presentation');
    expect(element.querySelector('[data-field="attribute"]')).toBeNull();
    const style = element.querySelector<HTMLSelectElement>('#demo-source-style')!;
    style.value = 'coastline-signal';
    style.dispatchEvent(new Event('change'));
    element.querySelector<HTMLButtonElement>('[data-action="map"]')!.click();

    expect(mapped).toHaveBeenCalledOnce();
    expect((mapped.mock.calls[0]![0] as CustomEvent).detail).toMatchObject({
      source: { id: vectorSource.id, format, attribution: 'Publisher dataset attribution' },
      style: 'coastline-signal',
    });
    expect((mapped.mock.calls[0]![0] as CustomEvent).detail).not.toHaveProperty('attributes');

    element.querySelector<HTMLButtonElement>('[data-action="reset"]')!.click();
    await settle();
    expect(api.remove).toHaveBeenCalledWith({
      ...vectorSource,
      attribution: 'Publisher dataset attribution',
    });
    expect(element.querySelector('#demo-source-url')).not.toBeNull();
  });
});

describe('custom-element lifecycle races', () => {
  it('cleans up a POST that succeeds after disconnect and does not enter a reconnected generation', async () => {
    const registration = deferred<DemoSourceResponse>();
    api.register.mockReturnValue(registration.promise);
    api.remove.mockResolvedValue(undefined);
    const element = mount();
    submit(element);

    element.remove();
    document.body.append(element);
    registration.resolve(source);
    await settle();

    expect(api.remove).toHaveBeenCalledWith(source);
    expect(element.querySelector('#demo-source-url')).not.toBeNull();
    expect(element.innerHTML).not.toContain('data.example.org');
  });

  it('ignores a stale POST error after disconnect and reconnect', async () => {
    const registration = deferred<DemoSourceResponse>();
    api.register.mockReturnValue(registration.promise);
    const element = mount();
    submit(element);

    element.remove();
    document.body.append(element);
    registration.reject(new Error('network'));
    await settle();

    expect(element.querySelector('#demo-source-url')).not.toBeNull();
    expect(element.textContent).not.toContain('503');
    expect(element.innerHTML).not.toContain('data.example.org');
  });

  it('reconciles a successful DELETE across disconnect and reconnect', async () => {
    api.register.mockResolvedValue(source);
    const removal = deferred<void>();
    api.remove.mockReturnValue(removal.promise);
    const element = mount();
    submit(element);
    await settle();
    element.querySelector<HTMLButtonElement>('[data-action="reset"]')!.click();

    element.remove();
    document.body.append(element);
    removal.resolve();
    await settle();

    expect(element.querySelector('#demo-source-url')).not.toBeNull();
    expect(element.textContent).not.toContain('Discarding…');
  });

  it('retains a retryable source when DELETE fails across disconnect and reconnect', async () => {
    api.register.mockResolvedValue(source);
    const removal = deferred<void>();
    api.remove.mockReturnValue(removal.promise);
    const element = mount();
    submit(element);
    await settle();
    element.querySelector<HTMLButtonElement>('[data-action="reset"]')!.click();

    element.remove();
    document.body.append(element);
    removal.reject(new Error('network'));
    await settle();

    expect(element.textContent).toContain('Retry removal');
    expect(element.querySelector<HTMLButtonElement>('[data-action="reset"]')!.disabled).toBe(false);
  });
});
