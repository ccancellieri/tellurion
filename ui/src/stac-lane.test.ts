/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { StacLaneDiscovery } from './stac-lane';

const response = (value: unknown) => new Response(JSON.stringify(value));
const directory = { links: [{ rel: 'stac', href: '/public/stac/catalogs/default' }] };
const landing = { links: [{ rel: 'search', href: '/search', type: 'application/geo+json' }] };
function mount() {
  const tab = document.createElement('button');
  tab.hidden = true;
  const status = document.createElement('p');
  const changed = vi.fn();
  document.body.append(tab, status);
  return { tab, status, changed, discovery: new StacLaneDiscovery(tab, status, changed) };
}
afterEach(() => { document.body.replaceChildren(); history.replaceState({}, '', '/'); vi.unstubAllGlobals(); });

describe('lazy advertised STAC lane', () => {
  it('does no discovery until requested, then reveals only a supported GET search', async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(response(directory)).mockResolvedValueOnce(response(landing));
    vi.stubGlobal('fetch', fetcher);
    const { tab, discovery } = mount();
    expect(tab.hidden).toBe(true);
    expect(fetcher).not.toHaveBeenCalled();
    await discovery.refresh();
    expect(tab.hidden).toBe(false);
    expect(discovery.endpoint?.searchHref).toBe('http://localhost:3000/search');
    await discovery.refresh();
    expect(fetcher).toHaveBeenCalledTimes(2);
  });
  it('keeps unadvertised capabilities hidden and provides explicit absence status', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response({ links: [] })));
    const { tab, status, discovery } = mount();
    await discovery.refresh();
    expect(tab.hidden).toBe(true);
    expect(status.textContent).toContain('not advertised');
  });
  it('offers a retry after a controlled discovery error', async () => {
    const fetcher = vi.fn().mockRejectedValueOnce(new Error('secret-error'))
      .mockResolvedValueOnce(response(directory)).mockResolvedValueOnce(response(landing));
    vi.stubGlobal('fetch', fetcher);
    const { tab, status, discovery } = mount();
    await discovery.refresh();
    expect(tab.hidden).toBe(true);
    expect(status.textContent).not.toContain('secret-error');
    status.querySelector('button')!.click();
    await vi.waitFor(() => expect(tab.hidden).toBe(false));
  });
  it('rechecks a changed context and removes the previously advertised capability', async () => {
    const fetcher = vi.fn().mockResolvedValueOnce(response(directory)).mockResolvedValueOnce(response(landing))
      .mockResolvedValueOnce(response({ links: [] }));
    vi.stubGlobal('fetch', fetcher);
    const { tab, changed, discovery } = mount();
    await discovery.refresh();
    history.replaceState({}, '', '/?tenant=city&catalog=missing');
    await discovery.refresh();
    expect(tab.hidden).toBe(true);
    expect(discovery.endpoint).toBeNull();
    expect(changed).toHaveBeenCalled();
    expect(fetcher.mock.calls[2][0]).toBe('http://localhost:3000/city');
  });
});
