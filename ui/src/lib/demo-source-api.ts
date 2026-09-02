import { demoSourceSelfHref, validateDemoSourceResponse, type DemoSourceResponse } from './demo-source';

export class DemoSourceApiError extends Error {
  readonly status: number;

  constructor(status: number) {
    super(`Source inspection was not accepted (${status}).`);
    this.status = status;
  }
}

export class DemoSourceMetadataError extends Error {
  constructor() {
    super('Source inspection returned invalid metadata.');
  }
}

function browserOrigin(): string {
  return globalThis.location?.origin || 'http://localhost';
}

async function readDemoResponse(response: Response): Promise<DemoSourceResponse> {
  if (!response.ok) throw new DemoSourceApiError(response.status);
  const source = validateDemoSourceResponse(await response.json(), browserOrigin());
  if (!source) throw new DemoSourceMetadataError();
  return source;
}

export async function registerDemoSource(
  url: string,
  signal?: AbortSignal,
): Promise<DemoSourceResponse> {
  const response = await fetch('/demo/sources', {
    method: 'POST',
    credentials: 'include',
    headers: { Accept: 'application/json', 'Content-Type': 'application/json' },
    body: JSON.stringify({ url }),
    signal,
  });
  return readDemoResponse(response);
}

export async function deleteDemoSource(source: DemoSourceResponse): Promise<void> {
  const selfHref = demoSourceSelfHref(source, browserOrigin());
  if (!selfHref) throw new DemoSourceMetadataError();
  const response = await fetch(selfHref, {
    method: 'DELETE',
    credentials: 'include',
    headers: { Accept: 'application/json' },
  });
  if (!response.ok && response.status !== 404) throw new DemoSourceApiError(response.status);
}
