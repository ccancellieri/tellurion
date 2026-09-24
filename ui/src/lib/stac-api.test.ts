import { afterEach, describe, expect, it, vi } from 'vitest';
import { discoverStac, fetchStacPage, stacSearchHref } from './stac-api';

const origin = 'https://tellurion.example';
const root = `${origin}/city/stac/catalogs/base`;
const searchHref = `${origin}/advertised-search?fixed=yes`;
const landing = { conformsTo: ['https://api.stacspec.org/v1.0.0/item-search'], links: [{ rel: 'search', href: searchHref, type: 'application/geo+json' }] };
function response(value: unknown): Response { return new Response(JSON.stringify(value)); }
function mockFetch(...values: unknown[]) {
  const fetcher = vi.fn();
  values.forEach((value) => fetcher.mockResolvedValueOnce(response(value)));
  vi.stubGlobal('fetch', fetcher);
  return fetcher;
}
afterEach(() => vi.unstubAllGlobals());

describe('STAC discovery and read-only search', () => {
  it('follows directory pagination and advertised search rather than constructing protocol paths', async () => {
    const fetcher = mockFetch({ links: [{ rel: 'next', href: '?token=next' }] },
      { links: [{ rel: 'stac', href: root }] }, landing);
    const signal = new AbortController().signal;
    expect(await discoverStac('?tenant=city&catalog=base', origin, signal)).toEqual({ tenantId: 'city', catalogId: 'base', searchHref });
    expect(fetcher.mock.calls.map(([href]) => href)).toEqual([`${origin}/city`, `${origin}/city?token=next`, root]);
    expect(fetcher.mock.calls[0][1]).toMatchObject({ credentials: 'same-origin', redirect: 'error' });
    expect(fetcher.mock.calls[0][1].signal.aborted).toBe(false);
  });
  it('does not fall back to an unrelated catalog', async () => {
    const fetcher = mockFetch({ links: [{ rel: 'stac', href: `${origin}/city/stac/catalogs/other` }] });
    expect(await discoverStac('?tenant=city&catalog=base', origin)).toBeNull();
    expect(fetcher).toHaveBeenCalledOnce();
  });
  it('rejects an explicitly empty context rather than opening the default catalog', async () => {
    const fetcher = mockFetch();
    await expect(discoverStac('?tenant=&catalog=base', origin)).rejects.toThrow();
    expect(fetcher).not.toHaveBeenCalled();
  });
  it.each([
    { method: 'POST' }, { headers: { Authorization: 'secret' } }, { body: {} }, { merge: true },
    { href: 'https://elsewhere.example/search' }, { href: 'https://user:secret@tellurion.example/search' },
    { href: 'javascript:alert(1)' }, { templated: true },
  ])('does not advertise an unsupported search link %j', async (override) => {
    mockFetch({ links: [{ rel: 'stac', href: root }] }, { ...landing, links: [{ ...landing.links[0], ...override }] });
    expect(await discoverStac('?tenant=city&catalog=base', origin)).toBeNull();
  });
  it('accepts an advertised GET search when conformance metadata is absent', async () => {
    mockFetch({ links: [{ rel: 'stac', href: root }] }, { ...landing, conformsTo: [] });
    expect(await discoverStac('?tenant=city&catalog=base', origin)).toEqual({ tenantId: 'city', catalogId: 'base', searchHref });
  });
  it('stops discovery after five directory pages', async () => {
    const fetcher = mockFetch(...Array.from({ length: 5 }, (_, i) => ({ links: [{ rel: 'next', href: `?page=${i + 1}` }] })));
    await expect(discoverStac('?tenant=city&catalog=base', origin)).rejects.toThrow('limit');
    expect(fetcher).toHaveBeenCalledTimes(5);
  });
  it('encodes supported filters and requests ten items', () => {
    const href = stacSearchHref(searchHref, { collections: 'roads,parks', ids: 'id & 1', datetime: '2026-01-01T00:00:00Z/..' });
    const params = new URL(href).searchParams;
    expect(params.get('limit')).toBe('10');
    expect(params.get('fixed')).toBe('yes');
    expect(params.get('collections')).toBe('roads,parks');
    expect(params.get('ids')).toBe('id & 1');
    expect(params.get('datetime')).toBe('2026-01-01T00:00:00Z/..');
  });
  it('normalizes ten items with bounded text and safe assets, retaining an opaque GET next link', async () => {
    const assets = Object.fromEntries(Array.from({ length: 18 }, (_, i) => [`asset-${i}`, { href: `https://assets.example/${i}.tif` }]));
    mockFetch({ type: 'FeatureCollection', features: Array.from({ length: 12 }, (_, i) => ({ type: 'Feature', id: `item-${i}`, properties: { datetime: '2026-01-01T00:00:00Z' }, assets })), links: [{ rel: 'next', href: '?token=opaque&limit=10' }] });
    const page = await fetchStacPage(searchHref, origin);
    expect(page.items).toHaveLength(10);
    expect(page.items[0].assets).toHaveLength(16);
    expect(page.items[0].id).toBe('item-0');
    expect(page.truncated).toBe(true);
    expect(page.nextHref).toBe(`${origin}/advertised-search?token=opaque&limit=10`);
  });
  it('omits unsafe asset navigation and does not pretend POST pagination is GET', async () => {
    const fetcher = mockFetch({ type: 'FeatureCollection', features: [{ id: '<img src=x>', properties: { datetime: null, start_datetime: 'start', end_datetime: 'end' }, assets: { bad: { href: 'javascript:alert(1)' }, credentials: { href: 'https://user:secret@assets.example/x' }, good: { href: '/asset.tif' } } }], links: [{ rel: 'next', href: '/next', method: 'POST', body: {} }] });
    const page = await fetchStacPage(searchHref, origin);
    expect(page.items[0].id).toBe('<img src=x>');
    expect(page.items[0].time).toBe('start / end');
    expect(page.items[0].assets).toEqual([{ title: 'good', href: `${origin}/asset.tif` }]);
    expect(page.nextHref).toBeNull();
    expect(page.unsupportedNext).toBe(true);
    expect(fetcher).toHaveBeenCalledOnce();
  });
  it('rejects invalid pages and oversized responses', async () => {
    mockFetch({ type: 'NotAFeatureCollection' }, { type: 'FeatureCollection', features: [], padding: 'x'.repeat(2_100_000) });
    await expect(fetchStacPage(searchHref, origin)).rejects.toThrow();
    await expect(fetchStacPage(searchHref, origin)).rejects.toThrow('limit');
  });
  it('reports excessive discovery links instead of falsely reporting an absent catalog', async () => {
    mockFetch({ links: Array.from({ length: 1001 }, () => ({ rel: 'other', href: '/other' })) });
    await expect(discoverStac('?tenant=city&catalog=base', origin)).rejects.toThrow('limit');
  });
  it('resolves relative assets against the item self link when available', async () => {
    mockFetch({ type: 'FeatureCollection', features: [{ id: 'scene', links: [{ rel: 'self', href: 'https://assets.example/scenes/scene.json' }], assets: { image: { href: './image.tif' } } }] });
    const page = await fetchStacPage(searchHref, origin);
    expect(page.items[0].assets[0].href).toBe('https://assets.example/scenes/image.tif');
  });
  it('aborts a request that exceeds the read deadline', async () => {
    vi.useFakeTimers();
    try {
      const fetcher = vi.fn((_href, options) => new Promise<Response>((_resolve, reject) => {
        options.signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')));
      }));
      vi.stubGlobal('fetch', fetcher);
      const result = fetchStacPage(searchHref, origin).catch((error: unknown) => error);
      await vi.advanceTimersByTimeAsync(15_000);
      expect(fetcher.mock.calls[0][1].signal?.aborted).toBe(true);
      expect(await result).toBeInstanceOf(Error);
    } finally { vi.useRealTimers(); }
  });
});
