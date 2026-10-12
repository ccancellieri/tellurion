/** @vitest-environment happy-dom */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import './stac-panel';
import type { TellurionStacPanel } from './stac-panel';

const endpoint = { tenantId: 'public', catalogId: 'default', searchHref: 'http://localhost:3000/search?fixed=yes' };
const item = { type: 'Feature', id: '<img src=x onerror=alert(1)>', collection: 'worldcover',
  links: [{ rel: 'self', href: '/items/worldcover-1' }],
  properties: { datetime: '2026-09-24T00:00:00Z', title: 'Published raster' },
  assets: { raster: { href: 'https://assets.example/raster.tif', type: 'image/tiff', roles: ['data'],
    title: '<script>unsafe</script>', description: 'Catalog supplied asset description' } } };
const json = (value: unknown) => new Response(JSON.stringify(value));
const page = (features: unknown[], links: unknown[] = []) => json({ type: 'FeatureCollection', features, links });
function mount(): TellurionStacPanel {
  const element = document.createElement('tellurion-stac-panel') as TellurionStacPanel;
  element.endpoint = endpoint;
  element.previewAvailable = true;
  document.body.append(element);
  return element;
}
const flush = async () => { await vi.waitFor(() => expect(document.querySelector('[data-field="status"]')?.textContent).not.toContain('Loading')); };
beforeEach(() => { history.replaceState({}, '', '/'); });
afterEach(() => { document.body.replaceChildren(); vi.unstubAllGlobals(); });

describe('STAC search inspector', () => {
  it('renders item IDs, time and safe asset links as text without fetching assets', async () => {
    const fetcher = vi.fn().mockResolvedValue(page([item]));
    vi.stubGlobal('fetch', fetcher);
    const element = mount();
    await flush();
    expect(element.textContent).toContain(item.id);
    expect(element.textContent).toContain('2026-09-24T00:00:00Z');
    expect(element.querySelector('img,script')).toBeNull();
    const asset = element.querySelector<HTMLAnchorElement>('[data-asset-link]');
    expect(asset?.href).toBe('https://assets.example/raster.tif');
    expect(asset?.rel).toContain('noreferrer');
    expect(asset?.textContent).toBe('<script>unsafe</script>');
    expect(fetcher).toHaveBeenCalledOnce();
  });
  it('explains unavailable temporary previews without exposing a preview action', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(page([item])));
    const element = mount();
    element.previewAvailable = false;
    await flush();
    expect(element.textContent).toContain('Temporary-source preview is unavailable');
    expect(element.querySelector('[data-action="preview-asset"]')).toBeNull();
  });
  it('emits one validated preview only after the explicit click and disables duplicates until completion', async () => {
    const fetcher = vi.fn().mockResolvedValue(page([item]));
    vi.stubGlobal('fetch', fetcher);
    const element = mount();
    const requests: CustomEvent[] = [];
    element.addEventListener('tellurion-stac-asset-preview', (event) => requests.push(event as CustomEvent));
    await flush();

    expect(requests).toHaveLength(0);
    expect(fetcher).toHaveBeenCalledOnce();
    const preview = element.querySelector<HTMLButtonElement>('[data-action="preview-asset"]')!;
    preview.click();
    preview.click();

    expect(requests).toHaveLength(1);
    expect(requests[0].detail).toMatchObject({
      requestId: expect.any(String),
      descriptor: {
        url: 'https://assets.example/raster.tif', format: 'cog',
        itemId: item.id, collectionId: 'worldcover', assetKey: 'raster',
        assetLabel: '<script>unsafe</script>', assetDescription: 'Catalog supplied asset description',
      },
    });
    expect(preview.disabled).toBe(true);
    expect(fetcher).toHaveBeenCalledOnce();

    element.dispatchEvent(new CustomEvent('tellurion-stac-asset-preview-complete', {
      detail: { requestId: requests[0].detail.requestId, result: 'accepted' },
    }));
    expect(preview.disabled).toBe(false);
    expect(element.textContent).toContain('source workflow');

    preview.click();
    element.dispatchEvent(new CustomEvent('tellurion-stac-asset-preview-complete', {
      detail: { requestId: requests[1].detail.requestId, result: 'busy' },
    }));
    expect(element.textContent).toContain('Discard the current temporary source');
    expect(preview.disabled).toBe(false);
  });
  it('clears a pending preview when disconnected so reconnect can issue a new request', async () => {
    vi.stubGlobal('fetch', vi.fn().mockImplementation(() => Promise.resolve(page([item]))));
    const element = mount();
    const requests: CustomEvent[] = [];
    element.addEventListener('tellurion-stac-asset-preview', (event) => requests.push(event as CustomEvent));
    await flush();

    element.querySelector<HTMLButtonElement>('[data-action="preview-asset"]')!.click();
    expect(requests).toHaveLength(1);
    element.remove();
    document.body.append(element);
    await vi.waitFor(() => expect(element.querySelector('[data-action="preview-asset"]')).not.toBeNull());

    const preview = element.querySelector<HTMLButtonElement>('[data-action="preview-asset"]')!;
    expect(preview.disabled).toBe(false);
    preview.click();
    expect(requests).toHaveLength(2);
  });
  it('shows unsupported and signed assets with fixed reasons and no preview action', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(page([{ ...item, assets: {
      signed: { href: 'https://assets.example/raster.tif?signature=secret', type: 'image/tiff', title: 'Signed raster' },
      pmtiles: { href: 'https://assets.example/map.pmtiles', type: 'application/vnd.pmtiles', title: 'PMTiles' },
    } }])));
    const element = mount();
    await flush();
    expect(element.textContent).toContain('Signed raster');
    expect(element.textContent).toContain('unsigned HTTPS');
    expect(element.textContent).toContain('PMTiles');
    expect(element.textContent).toContain('not recognized');
    expect(element.querySelector('[data-action="preview-asset"]')).toBeNull();
  });
  it('submits filters and replaces results when following the advertised next link', async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(page([item], [{ rel: 'next', href: '/opaque-next?cursor=x' }]))
      .mockResolvedValueOnce(page([{ ...item, id: 'second-page' }]))
      .mockResolvedValueOnce(page([]));
    vi.stubGlobal('fetch', fetcher);
    const element = mount();
    await flush();
    element.querySelector<HTMLButtonElement>('[data-action="next"]')!.click();
    await flush();
    expect(fetcher.mock.calls[1][0]).toBe('http://localhost:3000/opaque-next?cursor=x');
    expect(element.textContent).toContain('second-page');
    expect(element.textContent).not.toContain(item.id);
    element.querySelector<HTMLInputElement>('[name="collections"]')!.value = 'roads';
    element.querySelector<HTMLInputElement>('[name="ids"]')!.value = 'id-1';
    element.querySelector('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
    await flush();
    const url = new URL(fetcher.mock.calls[2][0]);
    expect(url.searchParams.get('collections')).toBe('roads');
    expect(url.searchParams.get('ids')).toBe('id-1');
    expect(url.searchParams.get('limit')).toBe('10');
    expect(element.textContent).toContain('No STAC items');
    expect(element.querySelector<HTMLButtonElement>('[data-action="next"]')?.disabled).toBe(true);
  });
  it('aborts previous searches and prevents stale responses from replacing current results', async () => {
    let resolveFirst!: (value: Response) => void;
    const fetcher = vi.fn().mockImplementationOnce(() => new Promise<Response>((resolve) => { resolveFirst = resolve; }))
      .mockResolvedValueOnce(page([{ ...item, id: 'current' }]));
    vi.stubGlobal('fetch', fetcher);
    const element = mount();
    element.querySelector('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
    await flush();
    expect(fetcher.mock.calls[0][1].signal.aborted).toBe(true);
    resolveFirst(page([{ ...item, id: 'stale' }]));
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(element.textContent).toContain('current');
    expect(element.textContent).not.toContain('stale');
    element.remove();
    expect(fetcher.mock.calls[1][1].signal.aborted).toBe(true);
  });
  it('reports unsupported pagination and controlled errors without leaking response details', async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(page([item], [{ rel: 'next', href: '/next', method: 'POST' }]))
      .mockRejectedValueOnce(new Error('private-token'));
    vi.stubGlobal('fetch', fetcher);
    const element = mount();
    await flush();
    expect(element.textContent).toContain('Next page requires an unsupported request');
    element.querySelector('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
    await flush();
    expect(element.textContent).toContain('Could not load STAC');
    expect(element.textContent).not.toContain('private-token');
  });
  it('rediscoveries the URL context without searching a previously selected catalog', async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(page([item])).mockResolvedValueOnce(json({ links: [] }));
    vi.stubGlobal('fetch', fetcher);
    const element = mount();
    await flush();
    history.replaceState({}, '', '/?tenant=city&catalog=missing');
    element.querySelector('form')!.dispatchEvent(new Event('submit', { cancelable: true }));
    await flush();
    expect(fetcher.mock.calls[1][0]).toBe('http://localhost:3000/city');
    expect(fetcher).toHaveBeenCalledTimes(2);
    expect(element.textContent).toContain('not advertised');
    expect(element.textContent).not.toContain(item.id);
  });
});
