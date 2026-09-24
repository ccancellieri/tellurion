/** @vitest-environment happy-dom */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import './stac-panel';
import type { TellurionStacPanel } from './stac-panel';

const endpoint = { tenantId: 'public', catalogId: 'default', searchHref: 'http://localhost:3000/search?fixed=yes' };
const item = { type: 'Feature', id: '<img src=x onerror=alert(1)>', properties: { datetime: '2026-09-24T00:00:00Z' }, assets: { raster: { href: 'https://assets.example/raster.tif', title: '<script>unsafe</script>' } } };
const json = (value: unknown) => new Response(JSON.stringify(value));
const page = (features: unknown[], links: unknown[] = []) => json({ type: 'FeatureCollection', features, links });
function mount(): TellurionStacPanel {
  const element = document.createElement('tellurion-stac-panel') as TellurionStacPanel;
  element.endpoint = endpoint;
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
    const asset = element.querySelector<HTMLAnchorElement>('[data-field="results"] a');
    expect(asset?.href).toBe('https://assets.example/raster.tif');
    expect(asset?.rel).toContain('noreferrer');
    expect(asset?.textContent).toBe('<script>unsafe</script>');
    expect(fetcher).toHaveBeenCalledOnce();
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
