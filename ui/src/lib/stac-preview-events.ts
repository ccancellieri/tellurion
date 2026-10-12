import type { StacAssetPreview } from './stac-api';

export const STAC_ASSET_PREVIEW_EVENT = 'tellurion-stac-asset-preview';
export const STAC_ASSET_PREVIEW_COMPLETE_EVENT = 'tellurion-stac-asset-preview-complete';

export type StacAssetPreviewResult = 'accepted' | 'busy' | 'invalid' | 'failed';

export interface StacAssetPreviewRequest {
  requestId: string;
  descriptor: StacAssetPreview;
}

export interface StacAssetPreviewCompletion {
  requestId: string;
  result: StacAssetPreviewResult;
}
