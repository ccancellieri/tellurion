import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ControlApiError,
  ControlEntityConflictError,
  ProductionControlReadClient,
} from './control-api';
import { PlatformSettingsEditSession } from './platform-settings-session';

afterEach(() => vi.unstubAllGlobals());

function json(value: unknown, status = 200, headers?: HeadersInit): Response {
  return new Response(JSON.stringify(value), { status, headers });
}

const session = { authenticated: true, principal: 'operator', csrf_token: 'csrf-secret', expires_in_s: 60 };
const settings = {
  control_revision: 7,
  entity_version: '5',
  resource: {
    cache_ttl_s: 30,
    stac: { providers: [{ name: 'Contact', url: 'https://example.test/contact' }] },
    future_setting: { nested: ['opaque'] },
  },
};
const preview = {
  base_revision: 7, prospective_revision: 8,
  changed_resources: ['platform'], entity_versions: { platform: '8' },
};
const commit = { revision: 8, changed_resources: ['platform'], replayed: false };

describe('durable platform settings client', () => {
  it('rejects malformed scoped settings IDs before a request', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    const client = new ProductionControlReadClient();

    await expect(client.rawSettings({ kind: 'tenant', tenant: 'a/b' })).rejects.toMatchObject({ status: 400 });
    await expect(client.rawSettings({ kind: 'catalog', tenant: 'tenant-a', catalog: '../b' })).rejects.toMatchObject({ status: 400 });
    await expect(client.rawSettings({ kind: 'tenant', tenant: undefined } as unknown as { kind: 'tenant'; tenant: string }))
      .rejects.toMatchObject({ status: 400 });
    await expect(client.rawSettings({ kind: 'platform', tenant: 'tenant-b' } as unknown as { kind: 'platform' }))
      .rejects.toMatchObject({ status: 400 });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('labels a denied tenant settings read without exposing response content', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({ detail: 'private setting' }, 403)));
    const client = new ProductionControlReadClient();

    await expect(client.rawSettings({ kind: 'tenant', tenant: 'tenant-a' })).rejects.toMatchObject({
      status: 403, message: 'You do not have access to this control resource.',
    });
  });

  it('labels an unavailable catalog settings read by scope', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({ detail: 'private setting' }, 503)));
    const client = new ProductionControlReadClient();

    await expect(client.rawSettings({ kind: 'catalog', tenant: 'tenant-a', catalog: 'cadastre' })).rejects.toMatchObject({
      status: 503, message: 'Catalog settings is unavailable.',
    });
  });
  it('reads the raw settings and entity version with cookies, without loading effective settings', async () => {
    const fetchMock = vi.fn().mockResolvedValue(json(settings, 200, { etag: '"control-entity-5"' }));
    vi.stubGlobal('fetch', fetchMock);

    const result = await new ProductionControlReadClient().platformSettings();

    expect(result).toEqual({
      controlRevision: 7, entityVersion: '5', resource: settings.resource,
    });
    expect(fetchMock).toHaveBeenCalledWith('/_control/v1/platform/settings', {
      method: 'GET', credentials: 'include', headers: { Accept: 'application/json' }, signal: undefined,
    });
    expect(fetchMock.mock.calls[0][1].headers).not.toHaveProperty('Authorization');
  });

  it('rejects a malformed durable envelope without exposing its body', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({ ...settings, entity_version: 5, detail: 'secret' })));

    await expect(new ProductionControlReadClient().platformSettings()).rejects.toMatchObject({
      status: 200, message: 'Platform settings is unavailable.',
    });
  });

  it('rejects a raw settings integer that JavaScript would round before editing', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(
      '{"control_revision":7,"entity_version":"5","resource":{"slow_request_ms":9007199254740993}}',
      { status: 200 },
    )));

    await expect(new ProductionControlReadClient().platformSettings()).rejects.toMatchObject({
      status: 200, message: 'Platform settings is unavailable.',
    });
  });

  it('accepts a safe boundary integer and a quoted opaque large number', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(
      '{"control_revision":7,"entity_version":"5","resource":{"slow_request_ms":9007199254740991,"stac":{"note":"escaped \\\" quote \\\\ and 9007199254740993"}}}',
      { status: 200 },
    )));

    const result = await new ProductionControlReadClient().platformSettings();
    expect(result.resource.slow_request_ms).toBe(Number.MAX_SAFE_INTEGER);
    expect(result.resource.stac).toEqual({ note: 'escaped " quote \\ and 9007199254740993' });
  });
});

describe('scoped settings edit session', () => {
  it.each([
    {
      scope: { kind: 'tenant', tenant: 'tenant-a' } as const,
      path: '/_control/v1/tenants/tenant-a/settings',
      key: 'tenant/tenant-a',
      operation: { SetTenantSettings: { tenant: 'tenant-a', settings: { ...settings.resource, cache_ttl_s: 60 } } },
    },
    {
      scope: { kind: 'catalog', tenant: 'tenant-a', catalog: 'cadastre' } as const,
      path: '/_control/v1/tenants/tenant-a/catalogs/cadastre/settings',
      key: 'tenant/tenant-a/catalog/cadastre',
      operation: { SetCatalogSettings: { tenant: 'tenant-a', catalog: 'cadastre', settings: { ...settings.resource, cache_ttl_s: 60 } } },
    },
  ])('uses only the $scope.kind settings endpoint and preserves opaque fields in frozen preview/apply', async ({ scope, path, key, operation }) => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json({ ...preview, changed_resources: [key], entity_versions: { [key]: '8' } }))
      .mockResolvedValueOnce(json({ ...commit, changed_resources: [key] }));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'scoped-id', scope);
    await editor.load();
    editor.editCacheTtlSeconds(60);
    await editor.preview();
    await editor.apply();

    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      '/_auth/control/session', path, `${path}?dry_run=true`, path,
    ]);
    expect(fetchMock.mock.calls[1][1]).toMatchObject({ method: 'GET', credentials: 'include' });
    const previewCall = fetchMock.mock.calls[2][1];
    const applyCall = fetchMock.mock.calls[3][1];
    expect(previewCall.body).toBe(applyCall.body);
    expect(JSON.parse(previewCall.body)).toEqual({
      idempotency_key: 'scoped-id',
      operations: [{ expected_entity_version: '5', operation }],
    });
    expect(previewCall.headers['x-tellurion-csrf']).toBe('csrf-secret');
    expect(applyCall.headers['x-tellurion-csrf']).toBe('csrf-secret');
  });

  it('rebases a tenant cache draft over newly read opaque fields after an entity conflict', async () => {
    const scope = { kind: 'tenant', tenant: 'tenant-a' } as const;
    const key = 'tenant/tenant-a';
    const newer = { ...settings, entity_version: '9', resource: { ...settings.resource, future_setting: { nested: ['updated'] } } };
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json({ ...preview, changed_resources: [key], entity_versions: { [key]: '8' } }))
      .mockResolvedValueOnce(json({ type: 'about:blank', title: 'Conflict', status: 409, code: 'ControlEntityVersionConflict' }, 409, { 'content-type': 'application/problem+json' }))
      .mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json(newer))
      .mockResolvedValueOnce(json({ ...preview, changed_resources: [key], entity_versions: { [key]: '10' } }));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'rebase-id', scope);
    await editor.load();
    editor.editCacheTtlSeconds(61);
    await editor.preview();
    await expect(editor.apply()).rejects.toBeInstanceOf(ControlEntityConflictError);
    await editor.rebaseAfterConflict();
    await editor.preview();

    expect(JSON.parse(fetchMock.mock.calls[6][1].body).operations[0]).toEqual({
      expected_entity_version: '9',
      operation: { SetTenantSettings: { tenant: 'tenant-a', settings: { ...newer.resource, cache_ttl_s: 61 } } },
    });
    expect(fetchMock.mock.calls[5][0]).toBe('/_control/v1/tenants/tenant-a/settings');
  });

  it('retries an uncertain catalog apply with the identical scoped request', async () => {
    const scope = { kind: 'catalog', tenant: 'tenant-a', catalog: 'cadastre' } as const;
    const key = 'tenant/tenant-a/catalog/cadastre';
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json({ ...preview, changed_resources: [key], entity_versions: { [key]: '8' } }))
      .mockRejectedValueOnce(new TypeError('network failed'))
      .mockResolvedValueOnce(json({ ...commit, changed_resources: [key], replayed: true }));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'retry-scoped-id', scope);
    await editor.load();
    editor.editCacheTtlSeconds(64);
    await editor.preview();
    await expect(editor.apply()).rejects.toMatchObject({ name: 'ControlUncertainWriteError' });
    await editor.retryUncertainApply();

    expect(fetchMock.mock.calls[3]).toEqual(fetchMock.mock.calls[4]);
    expect(fetchMock.mock.calls[4][0]).toBe('/_control/v1/tenants/tenant-a/catalogs/cadastre/settings');
    expect(editor.view()).toMatchObject({ phase: 'applied', commit: { replayed: true } });
  });
});

describe('platform settings edit session', () => {
  it('freezes a settings-only body for preview and apply while preserving opaque fields', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview))
      .mockResolvedValueOnce(json(commit));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'frozen-id');
    await editor.load();
    editor.editCacheTtlSeconds(60);

    await editor.preview();
    await editor.apply();

    const previewCall = fetchMock.mock.calls[2];
    const applyCall = fetchMock.mock.calls[3];
    expect(previewCall[0]).toBe('/_control/v1/platform/settings?dry_run=true');
    expect(applyCall[0]).toBe('/_control/v1/platform/settings');
    expect(previewCall[1].body).toBe(applyCall[1].body);
    expect(JSON.parse(previewCall[1].body)).toEqual({
      idempotency_key: 'frozen-id',
      operations: [{
        expected_entity_version: '5',
        operation: { SetPlatformSettings: { ...settings.resource, cache_ttl_s: 60 } },
      }],
    });
    for (const [, init] of [previewCall, applyCall]) {
      expect(init).toMatchObject({ method: 'PUT', credentials: 'include' });
      expect(init.headers).toMatchObject({
        Accept: 'application/json', 'Content-Type': 'application/json', 'x-tellurion-csrf': 'csrf-secret',
      });
      expect(init.headers).not.toHaveProperty('Authorization');
    }
    expect(editor.view()).toMatchObject({ phase: 'applied', draft: { cache_ttl_s: 60 }, commit: { revision: 8 } });
    expect(editor.view()).not.toHaveProperty('csrfToken');
  });

  it('forbids apply before preview and invalidates preview when the draft changes', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'first-id');
    await editor.load();
    editor.editCacheTtlSeconds(40);
    await expect(editor.apply()).rejects.toBeInstanceOf(ControlApiError);
    await editor.preview();
    editor.editCacheTtlSeconds(41);

    expect(editor.view()).toMatchObject({ phase: 'editing', draft: { cache_ttl_s: 41 } });
    expect(editor.view().preview).toBeUndefined();
    await expect(editor.apply()).rejects.toBeInstanceOf(ControlApiError);
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });

  it('keeps the draft but clears preview after a named entity conflict', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview))
      .mockResolvedValueOnce(json({
        type: 'about:blank', title: 'Conflict', status: 409, code: 'ControlEntityVersionConflict',
        detail: 'secret entity information',
      }, 409, { 'content-type': 'application/problem+json' }));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'conflict-id');
    await editor.load();
    editor.editCacheTtlSeconds(42);
    await editor.preview();

    await expect(editor.apply()).rejects.toBeInstanceOf(ControlEntityConflictError);
    expect(editor.view()).toMatchObject({ phase: 'editing', draft: { cache_ttl_s: 42 } });
    expect(editor.view().preview).toBeUndefined();
    await expect(editor.apply()).rejects.toBeInstanceOf(ControlApiError);
    expect(fetchMock).toHaveBeenCalledTimes(4);
  });

  it('blocks new writes after an uncertain apply and retries the identical request', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview))
      .mockRejectedValueOnce(new Error('network secret'))
      .mockResolvedValueOnce(json({ ...commit, replayed: true }));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'retry-id');
    await editor.load();
    editor.editCacheTtlSeconds(43);
    await editor.preview();

    await expect(editor.apply()).rejects.toMatchObject({ name: 'ControlUncertainWriteError' });
    expect(editor.view().phase).toBe('uncertain');
    expect(() => editor.editCacheTtlSeconds(44)).toThrow();
    await expect(editor.preview()).rejects.toBeInstanceOf(ControlApiError);
    await expect(editor.load()).rejects.toBeInstanceOf(ControlApiError);
    await editor.retryUncertainApply();

    expect(fetchMock.mock.calls[4]).toEqual(fetchMock.mock.calls[3]);
    expect(editor.view()).toMatchObject({ phase: 'applied', commit: { revision: 8, replayed: true } });
  });

  it('treats a malformed successful apply response as uncertain, not as permission for a new write', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview))
      .mockResolvedValueOnce(json({ detail: 'malformed-secret' }));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'malformed-id');
    await editor.load();
    editor.editCacheTtlSeconds(44);
    await editor.preview();

    await expect(editor.apply()).rejects.toMatchObject({ name: 'ControlUncertainWriteError' });
    expect(editor.view().phase).toBe('uncertain');
    expect(() => editor.editCacheTtlSeconds(45)).toThrow();
    expect(fetchMock).toHaveBeenCalledTimes(4);
  });

  it('requires an authenticated browser session with CSRF before sending a write', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json({ authenticated: true, principal: 'operator' }))
      .mockResolvedValueOnce(json(settings));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'no-csrf-id');
    await editor.load();
    editor.editCacheTtlSeconds(46);

    await expect(editor.preview()).rejects.toBeInstanceOf(ControlApiError);
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('blocks edits and a second load while the durable read is in flight', async () => {
    let finishRead!: (response: Response) => void;
    const pendingRead = new Promise<Response>((resolve) => { finishRead = resolve; });
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockReturnValueOnce(pendingRead);
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'read-id');

    const loading = editor.load();
    expect(editor.view().phase).toBe('loading');
    expect(() => editor.editCacheTtlSeconds(47)).toThrow();
    await expect(editor.preview()).rejects.toBeInstanceOf(ControlApiError);
    await expect(editor.load()).rejects.toBeInstanceOf(ControlApiError);
    finishRead(json(settings));
    await loading;

    expect(editor.view()).toMatchObject({ phase: 'editing', draft: { cache_ttl_s: 30 } });
    expect(fetchMock).toHaveBeenCalledTimes(2);
  });

  it('does not discard a frozen preview when a competing load is attempted', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview))
      .mockResolvedValueOnce(json(commit));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'single-id');
    await editor.load();
    editor.editCacheTtlSeconds(48);
    await editor.preview();

    await expect(editor.load()).rejects.toBeInstanceOf(ControlApiError);
    await editor.apply();

    expect(fetchMock).toHaveBeenCalledTimes(4);
    expect(fetchMock.mock.calls[3][1].body).toBe(fetchMock.mock.calls[2][1].body);
  });

  it('rebases a conflicted cache draft over fresh opaque settings before another preview', async () => {
    const newer = {
      control_revision: 10,
      entity_version: '9',
      resource: { ...settings.resource, stac: { providers: [{ name: 'Updated' }] }, future_setting: { nested: ['new'] } },
    };
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview))
      .mockResolvedValueOnce(json({
        type: 'about:blank', title: 'Conflict', status: 409, code: 'ControlEntityVersionConflict',
      }, 409, { 'content-type': 'application/problem+json' }))
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(newer))
      .mockResolvedValueOnce(json({ ...preview, base_revision: 10, prospective_revision: 11, entity_versions: { platform: '11' } }));
    vi.stubGlobal('fetch', fetchMock);
    const keys = vi.fn().mockReturnValueOnce('old-id').mockReturnValueOnce('new-id');
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), keys);
    await editor.load();
    editor.editCacheTtlSeconds(49);
    await editor.preview();
    await expect(editor.apply()).rejects.toBeInstanceOf(ControlEntityConflictError);
    await expect(editor.preview()).rejects.toBeInstanceOf(ControlApiError);

    await editor.rebaseAfterConflict();
    expect(editor.view()).toMatchObject({
      phase: 'editing', entityVersion: '9',
      draft: { cache_ttl_s: 49, stac: newer.resource.stac, future_setting: newer.resource.future_setting },
    });
    await editor.preview();
    expect(JSON.parse(fetchMock.mock.calls[6][1].body)).toEqual({
      idempotency_key: 'new-id',
      operations: [{ expected_entity_version: '9', operation: { SetPlatformSettings: { ...newer.resource, cache_ttl_s: 49 } } }],
    });
  });

  it('rebases a preview-time entity conflict before allowing another preview', async () => {
    const newer = { ...settings, control_revision: 10, entity_version: '9' };
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json({
        type: 'about:blank', title: 'Conflict', status: 409, code: 'ControlEntityVersionConflict',
      }, 409, { 'content-type': 'application/problem+json' }))
      .mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json(newer))
      .mockResolvedValueOnce(json({ ...preview, base_revision: 10, prospective_revision: 11, entity_versions: { platform: '11' } }));
    vi.stubGlobal('fetch', fetchMock);
    const editor = new PlatformSettingsEditSession(new ProductionControlReadClient(), () => 'preview-conflict-id');
    await editor.load();
    editor.editCacheTtlSeconds(50);

    await expect(editor.preview()).rejects.toBeInstanceOf(ControlEntityConflictError);
    expect(editor.view()).toMatchObject({ phase: 'editing', needsRebase: true, draft: { cache_ttl_s: 50 } });
    await editor.rebaseAfterConflict();
    await editor.preview();

    expect(JSON.parse(fetchMock.mock.calls[5][1].body).operations[0].expected_entity_version).toBe('9');
  });
});
