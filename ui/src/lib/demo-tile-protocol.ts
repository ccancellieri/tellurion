export const DEMO_TILE_PROTOCOL = 'tellurion-demo';

const DEFAULT_MAX_CONCURRENT_OPERATIONS = 2;
const SOURCE_ID = /^[A-Za-z0-9_-]+$/;
const PROTOCOL_SCOPE = /^\/(map-[1-9][0-9]*)\//;
let nextProtocolScope = 0;

export type DemoTileLoader = (url: string, signal: AbortSignal) => Promise<ArrayBuffer>;

interface QueuedTile {
  generation: number;
  url: string;
  signal: AbortSignal;
  state: 'queued' | 'running';
  controller: AbortController | null;
  settled: boolean;
  resolve: (data: ArrayBuffer) => void;
  reject: (error: unknown) => void;
  abort: () => void;
}

function cancelled(): Error {
  const error = new Error('Tile request cancelled.');
  error.name = 'AbortError';
  return error;
}

function operationLimit(advertised: number | undefined): number {
  return typeof advertised === 'number' && Number.isInteger(advertised) && advertised > 0
    ? Math.min(advertised, DEFAULT_MAX_CONCURRENT_OPERATIONS)
    : DEFAULT_MAX_CONCURRENT_OPERATIONS;
}

export function demoTileProtocolScope(protocolUrl: string): string | null {
  try {
    const url = new URL(protocolUrl);
    if (
      url.protocol !== `${DEMO_TILE_PROTOCOL}:` || url.host || url.username ||
      url.password || url.search || url.hash
    ) return null;
    return url.pathname.match(PROTOCOL_SCOPE)?.[1] ?? null;
  } catch {
    return null;
  }
}

async function fetchTile(url: string, signal: AbortSignal): Promise<ArrayBuffer> {
  const response = await fetch(url, {
    credentials: 'include',
    headers: { Accept: 'image/png' },
    signal,
  });
  if (!response.ok) throw new Error(`Tile request was not accepted (${response.status}).`);
  return response.arrayBuffer();
}

/** Owns the short-lived raster request queue for the active public-demo map.
 * It only translates a validated, same-origin demo route; remote source URLs
 * never reach MapLibre or this queue. */
export class DemoTileProtocol {
  readonly scopeId = `map-${++nextProtocolScope}`;
  readonly #loader: DemoTileLoader;
  #generation = 0;
  #limit = DEFAULT_MAX_CONCURRENT_OPERATIONS;
  #origin: string | null = null;
  #sourceId: string | null = null;
  #pending: QueuedTile[] = [];
  #running = new Set<QueuedTile>();

  constructor(loader: DemoTileLoader = fetchTile) {
    this.#loader = loader;
  }

  activate(
    sourceId: string,
    origin: string,
    httpsTemplate: string,
    maxConcurrentOperations?: number,
  ): string | null {
    this.clear();
    if (!SOURCE_ID.test(sourceId)) return null;
    let canonicalOrigin: string;
    try {
      canonicalOrigin = new URL(origin).origin;
    } catch {
      return null;
    }
    const path = `/demo/sources/${sourceId}/tiles/WebMercatorQuad/{z}/{y}/{x}.png`;
    if (httpsTemplate !== `${canonicalOrigin}${path}`) return null;
    this.#sourceId = sourceId;
    this.#origin = canonicalOrigin;
    this.#limit = operationLimit(maxConcurrentOperations);
    return `${DEMO_TILE_PROTOCOL}:///${this.scopeId}/${this.#generation}${path}`;
  }

  load(protocolUrl: string, signal: AbortSignal): Promise<ArrayBuffer> {
    const url = this.#httpsUrl(protocolUrl);
    if (!url) return Promise.reject(new Error('Tile request is outside the active demo source.'));
    if (signal.aborted) return Promise.reject(cancelled());
    return new Promise<ArrayBuffer>((resolve, reject) => {
      const tile: QueuedTile = {
        generation: this.#generation,
        url,
        signal,
        state: 'queued',
        controller: null,
        settled: false,
        resolve,
        reject,
        abort: () => this.#abort(tile),
      };
      signal.addEventListener('abort', tile.abort, { once: true });
      this.#pending.push(tile);
      this.#drain();
    });
  }

  clear(): void {
    this.#generation += 1;
    this.#origin = null;
    this.#sourceId = null;
    for (const tile of this.#pending.splice(0)) this.#reject(tile, cancelled());
    for (const tile of [...this.#running]) {
      this.#running.delete(tile);
      tile.controller?.abort();
      this.#reject(tile, cancelled());
    }
  }

  #httpsUrl(protocolUrl: string): string | null {
    if (!this.#origin || !this.#sourceId) return null;
    try {
      const url = new URL(protocolUrl);
      const activationPrefix = `/${this.scopeId}/${this.#generation}`;
      const activationPath = `${activationPrefix}/demo/sources/${this.#sourceId}`;
      const route = new RegExp(
        `^${activationPath}/tiles/WebMercatorQuad/[0-9]+/[0-9]+/[0-9]+\\.png$`,
      );
      if (
        url.protocol !== `${DEMO_TILE_PROTOCOL}:` || url.host || url.username ||
        url.password || url.search || url.hash || !route.test(url.pathname)
      ) return null;
      return `${this.#origin}${url.pathname.slice(activationPrefix.length)}`;
    } catch {
      return null;
    }
  }

  #abort(tile: QueuedTile): void {
    if (tile.settled) return;
    if (tile.state === 'queued') {
      const index = this.#pending.indexOf(tile);
      if (index >= 0) this.#pending.splice(index, 1);
    } else {
      this.#running.delete(tile);
      tile.controller?.abort();
    }
    this.#reject(tile, cancelled());
    if (tile.generation === this.#generation) this.#drain();
  }

  #drain(): void {
    while (this.#running.size < this.#limit && this.#pending.length > 0) {
      const tile = this.#pending.shift()!;
      if (tile.settled || tile.signal.aborted || tile.generation !== this.#generation) {
        this.#reject(tile, cancelled());
        continue;
      }
      tile.state = 'running';
      tile.controller = new AbortController();
      this.#running.add(tile);
      void Promise.resolve()
        .then(() => {
          if (tile.settled || tile.generation !== this.#generation) throw cancelled();
          return this.#loader(tile.url, tile.controller!.signal);
        })
        .then(
          (data) => {
            if (!tile.settled && tile.generation === this.#generation) this.#resolve(tile, data);
          },
          (error) => {
            if (!tile.settled) this.#reject(tile, error);
          },
        )
        .finally(() => {
          this.#running.delete(tile);
          if (tile.generation === this.#generation) this.#drain();
        });
    }
  }

  #resolve(tile: QueuedTile, data: ArrayBuffer): void {
    tile.settled = true;
    tile.signal.removeEventListener('abort', tile.abort);
    tile.resolve(data);
  }

  #reject(tile: QueuedTile, error: unknown): void {
    if (tile.settled) return;
    tile.settled = true;
    tile.signal.removeEventListener('abort', tile.abort);
    tile.reject(error);
  }
}
