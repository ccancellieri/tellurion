interface TileContent {
  contentUrl: string;
  content: unknown;
  hasEmptyContent: boolean;
}

/** Adapt the server's empty-tile response before loaders.gl accounts for GPU memory. */
export function createEmptyTileAdapter(
  tileUrls: readonly string[],
  signal: AbortSignal,
  fetcher: (url: string, options?: RequestInit) => Promise<Response> = fetch,
): {
  fetch: (url: string, options?: RequestInit) => Promise<Response>;
  contentLoader: (tile: TileContent) => Promise<void>;
} {
  const allowed = new Set(tileUrls);
  const empty = new Set<string>();
  return {
    async fetch(url, options) {
      empty.delete(url);
      const response = await fetcher(url, { ...options, signal });
      empty.delete(url);
      if (!signal.aborted && allowed.has(url) && response.status === 204) empty.add(url);
      return response;
    },
    async contentLoader(tile) {
      if (!signal.aborted && tile.content === null && empty.has(tile.contentUrl)) {
        tile.hasEmptyContent = true;
        tile.content = { byteLength: 0, gpuMemoryUsageInBytes: 0 };
      }
    },
  };
}
