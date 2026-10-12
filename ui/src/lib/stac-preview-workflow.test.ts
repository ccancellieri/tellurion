/** @vitest-environment happy-dom */
import { describe, expect, it, vi } from 'vitest';
import type { StacAssetPreview } from './stac-api';
import { connectStacPreviewWorkflow, demoSourcePreviewAvailable } from './stac-preview-workflow';

const descriptor: StacAssetPreview = {
  url: 'https://assets.example/worldcover.tif', format: 'cog',
  itemId: 'worldcover-2021', collectionId: 'worldcover', assetKey: 'land-cover',
  assetLabel: 'WorldCover', mediaType: 'image/tiff', roles: ['data'],
  itemHref: 'https://catalog.example/items/worldcover-2021', assetDescription: null,
};

describe('operator STAC preview composition', () => {
  it.each([[405, true], [404, false]])(
    'treats GET /demo/sources status %i as availability %s',
    async (status, expected) => {
      const fetcher = vi.fn().mockResolvedValue(new Response(null, { status }));
      await expect(demoSourcePreviewAvailable(fetcher)).resolves.toBe(expected);
      expect(fetcher).toHaveBeenCalledWith('/demo/sources', {
        method: 'GET', credentials: 'same-origin', redirect: 'error',
        headers: { Accept: 'application/json' }, signal: expect.any(AbortSignal),
      });
    },
  );

  it('treats an unresolved availability probe as unavailable after its deadline', async () => {
    vi.useFakeTimers();
    const fetcher = vi.fn().mockImplementation((_url, init: RequestInit) => new Promise((_resolve, reject) => {
      init.signal?.addEventListener('abort', () => reject(new DOMException('aborted', 'AbortError')));
    }));
    const available = demoSourcePreviewAvailable(fetcher);
    await vi.advanceTimersByTimeAsync(15_000);
    await expect(available).resolves.toBe(false);
    vi.useRealTimers();
  });

  it('forwards one bounded panel request to the existing workflow and returns matching completion', async () => {
    const container = document.createElement('section');
    const panel = document.createElement('div');
    container.append(panel);
    const previewStacAsset = vi.fn().mockResolvedValue('accepted');
    const completions: CustomEvent[] = [];
    panel.addEventListener('tellurion-stac-asset-preview-complete', (event) => completions.push(event as CustomEvent));
    const disconnect = connectStacPreviewWorkflow(container, { previewStacAsset });

    panel.dispatchEvent(new CustomEvent('tellurion-stac-asset-preview', {
      bubbles: true,
      detail: { requestId: 'stac-preview-1', descriptor },
    }));
    await vi.waitFor(() => expect(completions).toHaveLength(1));

    expect(previewStacAsset).toHaveBeenCalledOnce();
    expect(previewStacAsset).toHaveBeenCalledWith(descriptor);
    expect(completions[0].detail).toEqual({ requestId: 'stac-preview-1', result: 'accepted' });
    disconnect();
  });

  it('rejects an unbounded request id without calling the workflow', async () => {
    const container = document.createElement('section');
    const panel = document.createElement('div');
    container.append(panel);
    const previewStacAsset = vi.fn();
    connectStacPreviewWorkflow(container, { previewStacAsset });
    panel.dispatchEvent(new CustomEvent('tellurion-stac-asset-preview', {
      bubbles: true,
      detail: { requestId: `stac-preview-${'1'.repeat(100)}`, descriptor },
    }));
    await Promise.resolve();
    expect(previewStacAsset).not.toHaveBeenCalled();
  });
});
