import {
  STAC_ASSET_PREVIEW_COMPLETE_EVENT,
  STAC_ASSET_PREVIEW_EVENT,
  type StacAssetPreviewCompletion,
  type StacAssetPreviewRequest,
  type StacAssetPreviewResult,
} from './stac-preview-events';

interface StacPreviewWorkflow {
  previewStacAsset(value: unknown): Promise<StacAssetPreviewResult>;
}

const REQUEST_ID = /^stac-preview-[1-9][0-9]{0,9}$/;

export async function demoSourcePreviewAvailable(fetcher: typeof fetch = fetch): Promise<boolean> {
  const request = new AbortController();
  const deadline = window.setTimeout(() => request.abort(), 15_000);
  try {
    const response = await fetcher('/demo/sources', {
      method: 'GET',
      credentials: 'same-origin',
      redirect: 'error',
      headers: { Accept: 'application/json' },
      signal: request.signal,
    });
    return response.status === 405;
  } catch {
    return false;
  } finally {
    window.clearTimeout(deadline);
  }
}

export function connectStacPreviewWorkflow(
  container: EventTarget,
  workflow: StacPreviewWorkflow,
): () => void {
  const receive = (event: Event): void => {
    if (!(event instanceof CustomEvent) || !event.target) return;
    const detail = event.detail as Partial<StacAssetPreviewRequest> | null;
    if (!detail || typeof detail.requestId !== 'string' || !REQUEST_ID.test(detail.requestId)) return;
    const target = event.target;
    void workflow.previewStacAsset(detail.descriptor).then(
      (result) => complete(target, detail.requestId!, result),
      () => complete(target, detail.requestId!, 'failed'),
    );
  };
  container.addEventListener(STAC_ASSET_PREVIEW_EVENT, receive);
  return () => container.removeEventListener(STAC_ASSET_PREVIEW_EVENT, receive);
}

function complete(target: EventTarget, requestId: string, result: StacAssetPreviewResult): void {
  const detail: StacAssetPreviewCompletion = { requestId, result };
  target.dispatchEvent(new CustomEvent(STAC_ASSET_PREVIEW_COMPLETE_EVENT, { detail }));
}
