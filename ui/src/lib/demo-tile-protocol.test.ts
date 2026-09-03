import { describe, expect, it, vi } from 'vitest';
import { DemoTileProtocol } from './demo-tile-protocol';

const ORIGIN = 'https://tellurion.example';

interface PendingTile {
  url: string;
  signal: AbortSignal;
  resolve: (data: ArrayBuffer) => void;
  reject: (error: Error) => void;
}

function pendingLoader(): {
  pending: PendingTile[];
  load: (url: string, signal: AbortSignal) => Promise<ArrayBuffer>;
} {
  const pending: PendingTile[] = [];
  return {
    pending,
    load: (url, signal) => new Promise((resolve, reject) => {
      pending.push({ url, signal, resolve, reject });
    }),
  };
}

function activate(
  protocol: DemoTileProtocol,
  sourceId = 'source-one',
  maxConcurrentOperations: number | undefined = 2,
): string {
  const template = protocol.activate(
    sourceId,
    ORIGIN,
    `${ORIGIN}/demo/sources/${sourceId}/tiles/WebMercatorQuad/{z}/{y}/{x}.png`,
    maxConcurrentOperations,
  );
  expect(template).toContain(`/demo/sources/${sourceId}/tiles/WebMercatorQuad/{z}/{y}/{x}.png`);
  return template!;
}

function tile(template: string, column: number): string {
  return template.replace('{z}', '0').replace('{y}', '0').replace('{x}', String(column));
}

describe('public-demo raster tile protocol', () => {
  it('keeps an advertised tile burst at two active operations', async () => {
    const loader = pendingLoader();
    const protocol = new DemoTileProtocol(loader.load);
    const template = activate(protocol);

    const first = protocol.load(tile(template, 0), new AbortController().signal);
    const second = protocol.load(tile(template, 1), new AbortController().signal);
    const third = protocol.load(tile(template, 2), new AbortController().signal);
    await vi.waitFor(() => expect(loader.pending).toHaveLength(2));

    loader.pending[0].resolve(new ArrayBuffer(1));
    await expect(first).resolves.toHaveProperty('byteLength', 1);
    await vi.waitFor(() => expect(loader.pending).toHaveLength(3));

    loader.pending[1].resolve(new ArrayBuffer(2));
    loader.pending[2].resolve(new ArrayBuffer(3));
    await expect(Promise.all([second, third])).resolves.toEqual([
      expect.objectContaining({ byteLength: 2 }),
      expect.objectContaining({ byteLength: 3 }),
    ]);
  });

  it('starts the next queued tile after an active tile fails', async () => {
    const loader = pendingLoader();
    const protocol = new DemoTileProtocol(loader.load);
    const template = activate(protocol);

    const failed = protocol.load(tile(template, 0), new AbortController().signal);
    const second = protocol.load(tile(template, 1), new AbortController().signal);
    const third = protocol.load(tile(template, 2), new AbortController().signal);
    await vi.waitFor(() => expect(loader.pending).toHaveLength(2));

    loader.pending[0].reject(new Error('tile failed'));
    await expect(failed).rejects.toThrow('tile failed');
    await vi.waitFor(() => expect(loader.pending).toHaveLength(3));

    loader.pending[1].resolve(new ArrayBuffer(1));
    loader.pending[2].resolve(new ArrayBuffer(1));
    await expect(Promise.all([second, third])).resolves.toHaveLength(2);
  });

  it('removes a cancelled queued tile without blocking later work', async () => {
    const loader = pendingLoader();
    const protocol = new DemoTileProtocol(loader.load);
    const template = activate(protocol, 'source-one', 1);
    const first = protocol.load(tile(template, 0), new AbortController().signal);
    const queuedController = new AbortController();
    const cancelled = protocol.load(tile(template, 1), queuedController.signal);
    await vi.waitFor(() => expect(loader.pending).toHaveLength(1));

    queuedController.abort();
    await expect(cancelled).rejects.toMatchObject({ name: 'AbortError' });
    loader.pending[0].resolve(new ArrayBuffer(1));
    await first;

    const later = protocol.load(tile(template, 2), new AbortController().signal);
    await vi.waitFor(() => expect(loader.pending).toHaveLength(2));
    loader.pending[1].resolve(new ArrayBuffer(1));
    await expect(later).resolves.toHaveProperty('byteLength', 1);
  });

  it('releases an active slot when an aborted loader does not settle', async () => {
    const loader = pendingLoader();
    const protocol = new DemoTileProtocol(loader.load);
    const template = activate(protocol, 'source-one', 1);
    const activeController = new AbortController();
    const active = protocol.load(tile(template, 0), activeController.signal);
    const activeRejection = expect(active).rejects.toMatchObject({ name: 'AbortError' });
    const queued = protocol.load(tile(template, 1), new AbortController().signal);
    await vi.waitFor(() => expect(loader.pending).toHaveLength(1));

    activeController.abort();
    await activeRejection;
    await vi.waitFor(() => expect(loader.pending).toHaveLength(2));

    loader.pending[1].resolve(new ArrayBuffer(1));
    await expect(queued).resolves.toHaveProperty('byteLength', 1);
    loader.pending[0].resolve(new ArrayBuffer(1));
  });

  it('aborts old work and never returns it after the map changes source', async () => {
    const loader = pendingLoader();
    const protocol = new DemoTileProtocol(loader.load);
    const oldTemplate = activate(protocol, 'source-one', 1);
    const oldActive = protocol.load(tile(oldTemplate, 0), new AbortController().signal);
    const oldQueued = protocol.load(tile(oldTemplate, 1), new AbortController().signal);
    const activeRejection = expect(oldActive).rejects.toMatchObject({ name: 'AbortError' });
    const queuedRejection = expect(oldQueued).rejects.toMatchObject({ name: 'AbortError' });
    await vi.waitFor(() => expect(loader.pending).toHaveLength(1));

    const currentTemplate = activate(protocol, 'source-two', 1);
    await Promise.all([activeRejection, queuedRejection]);
    expect(loader.pending[0].signal.aborted).toBe(true);

    const current = protocol.load(tile(currentTemplate, 0), new AbortController().signal);
    await vi.waitFor(() => expect(loader.pending).toHaveLength(2));
    loader.pending[0].resolve(new ArrayBuffer(9));
    loader.pending[1].resolve(new ArrayBuffer(2));
    await expect(current).resolves.toHaveProperty('byteLength', 2);
  });

  it('defaults an unavailable advertised limit to the two-operation contract', async () => {
    const loader = pendingLoader();
    const protocol = new DemoTileProtocol(loader.load);
    const template = activate(protocol, 'source-one', undefined);

    const requests = [0, 1, 2].map((column) =>
      protocol.load(tile(template, column), new AbortController().signal));
    await vi.waitFor(() => expect(loader.pending).toHaveLength(2));
    loader.pending[0].resolve(new ArrayBuffer(1));
    await vi.waitFor(() => expect(loader.pending).toHaveLength(3));
    loader.pending[1].resolve(new ArrayBuffer(1));
    loader.pending[2].resolve(new ArrayBuffer(1));
    await Promise.all(requests);
  });

  it('rejects a late request from an earlier activation of the same source', async () => {
    const protocol = new DemoTileProtocol(async () => new ArrayBuffer(1));
    const oldTemplate = activate(protocol);
    const currentTemplate = activate(protocol);

    expect(currentTemplate).not.toBe(oldTemplate);
    await expect(protocol.load(tile(oldTemplate, 0), new AbortController().signal))
      .rejects.toThrow('outside the active demo source');
  });
});
