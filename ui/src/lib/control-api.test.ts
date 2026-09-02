import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ControlApiError,
  ControlConflictError,
  ControlForbiddenError,
  ControlScopeAbsentError,
  ControlSignInRequiredError,
  ProductionControlReadClient,
} from './control-api';
import { FixtureControlReadClient } from './control-fixtures';

afterEach(() => vi.unstubAllGlobals());

function json(value: unknown, status = 200, headers?: HeadersInit): Response {
  return new Response(JSON.stringify(value), { status, headers });
}

const overview = {
  scope: 'self', store_revision: 8, applied_revision: 7, lag: 1,
  last_successful_refresh_unix_ms: 1234, poll_failures: 0, activation_failures: 0,
  config_version: 'revision-7',
};

const tenantPage = {
  control_revision: 8,
  items: [{
    control_revision: 8, entity_version: '3', resource: {
      id: 'tenant-a', settings: {}, tombstoned: false,
    },
  }],
  next_after: 'tenant-a',
};

const effectiveSettings = {
  applied_revision: 7,
  effective: {
    node: { level: 'platform' },
    settings: {
      cache_ttl_s: { value: 30, provenance: { kind: 'local_override' } },
      stac: { value: null, provenance: { kind: 'inherited', level: 'platform' } },
      protocols: { value: null, provenance: { kind: 'profile', level: 'tenant', profile_id: 'fast' } },
    },
  },
};

const auditPage = {
  revision: 8,
  items: [{
    revision: 8,
    actor: { issuer: 'https://issuer.example', subject: 'operator' },
    method: 'PUT', canonical_path: '/_control/v1/platform/settings',
    correlation_id: 'correlation-1', changed_resources: ['platform'],
    recorded_at_unix_ms: 1234, applying_instance: 'instance-a',
  }],
  next_after: 8,
};

describe('production control read client', () => {
  it('production break: requests same-origin root resources with browser credentials and JSON accept headers', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json({ authenticated: true, principal: 'operator', csrf_token: 'csrf-secret', expires_in_s: 60 }))
      .mockResolvedValueOnce(json(overview))
      .mockResolvedValueOnce(json(tenantPage))
      .mockResolvedValueOnce(json(effectiveSettings))
      .mockResolvedValueOnce(json(auditPage));
    vi.stubGlobal('fetch', fetchMock);

    const client = new ProductionControlReadClient();
    await client.session();
    await client.overview();
    await client.tenants();
    await client.effectiveSettings();
    await client.audit();

    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
      '/_auth/control/session',
      '/_control/v1/platform/overview',
      '/_control/v1/tenants',
      '/_control/v1/platform/effective-settings',
      '/_control/v1/platform/audit',
    ]);
    for (const [, init] of fetchMock.mock.calls) {
      expect(init).toEqual({
        method: 'GET', credentials: 'include', headers: { Accept: 'application/json' }, signal: undefined,
      });
      expect((init as RequestInit).headers).not.toHaveProperty('Authorization');
    }
  });

  it('production break: preserves a server tenant cursor without constructing a resource path', async () => {
    const fetchMock = vi.fn().mockResolvedValue(json(tenantPage));
    vi.stubGlobal('fetch', fetchMock);

    const page = await new ProductionControlReadClient().tenants('tenant-a/next');

    expect(fetchMock).toHaveBeenCalledWith('/_control/v1/tenants?after=tenant-a%2Fnext', expect.any(Object));
    expect(page.nextAfter).toBe('tenant-a');
  });

  it('production break: preserves a server audit cursor without exposing query values in errors', async () => {
    const secretCursor = 'cursor-secret';
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({ detail: `do not reveal ${secretCursor}` }, 500, {
      'content-type': 'application/problem+json',
    })));

    await expect(new ProductionControlReadClient().audit(secretCursor)).rejects.toThrow('Audit log is unavailable.');
    await expect(new ProductionControlReadClient().audit(secretCursor)).rejects.not.toThrow(secretCursor);
  });

  it('production break: decodes server setting values with only the documented provenance variants', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json(effectiveSettings)));

    const view = await new ProductionControlReadClient().effectiveSettings();

    expect(view.appliedRevision).toBe(7);
    expect((view.effective as { node?: { level?: string } }).node?.level).toBe('platform');
    expect((view.effective as { settings?: { cache_ttl_s?: { value?: number; provenance?: { kind?: string } } } })
      .settings?.cache_ttl_s).toEqual({ value: 30, provenance: { kind: 'local_override' } });
  });

  it('production break: accepts long non-empty NodeRef and profile identifiers from the server contract', async () => {
    const profileId = 'profile-'.repeat(80);
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({
      applied_revision: 7,
      effective: {
        node: { level: 'collection', tenant: 't'.repeat(300), catalog: 'c'.repeat(300), collection: 'd'.repeat(300) },
        settings: {
          protocols: { value: null, provenance: { kind: 'profile', level: 'tenant', profile_id: profileId } },
        },
      },
    })));

    const view = await new ProductionControlReadClient().effectiveSettings();

    expect(view.effective.node).toEqual({
      level: 'collection', tenant: 't'.repeat(300), catalog: 'c'.repeat(300), collection: 'd'.repeat(300),
    });
    expect(view.effective.settings.protocols.provenance).toEqual({
      kind: 'profile', level: 'tenant', profileId,
    });
  });

  it('production break: rejects empty profile ids and invalid NodeRef level combinations', async () => {
    const invalidNodes = [
      { level: 'platform', tenant: 'tenant-a' },
      { level: 'tenant' },
      { level: 'tenant', tenant: '' },
      { level: 'tenant', tenant: 'tenant-a', catalog: 'catalog-a' },
      { level: 'catalog', tenant: 'tenant-a' },
      { level: 'catalog', tenant: 'tenant-a', catalog: 'catalog-a', collection: 'collection-a' },
      { level: 'collection', tenant: 'tenant-a', catalog: 'catalog-a' },
    ];
    const responses = [
      ...invalidNodes.map((node) => json({
        applied_revision: 7,
        effective: { node, settings: effectiveSettings.effective.settings },
      })),
      json({
        applied_revision: 7,
        effective: {
          node: { level: 'platform' },
          settings: { protocols: { value: null, provenance: { kind: 'profile', level: 'tenant', profile_id: '' } } },
        },
      }),
    ];
    vi.stubGlobal('fetch', vi.fn()
      .mockImplementation(() => Promise.resolve(responses.shift()!)));
    const client = new ProductionControlReadClient();

    for (let index = 0; index < invalidNodes.length + 1; index += 1) {
      await expect(client.effectiveSettings()).rejects.toMatchObject({
        status: 200, message: 'Effective settings is unavailable.',
      });
    }
  });

  it('production break: converts transport failures to stable errors without leaking a cursor', async () => {
    const cursor = 'cursor-secret';
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error(`network failed for after=${cursor}`)));

    await expect(new ProductionControlReadClient().audit('8')).rejects.toMatchObject({
      status: 0, message: 'Audit log is unavailable.',
    });
    await expect(new ProductionControlReadClient().audit('8')).rejects.not.toThrow(cursor);
  });

  it('production break: round-trips a safe server audit cursor as a decimal string', async () => {
    const fetchMock = vi.fn()
      .mockResolvedValueOnce(json(auditPage))
      .mockResolvedValueOnce(json({ ...auditPage, next_after: null }));
    vi.stubGlobal('fetch', fetchMock);
    const client = new ProductionControlReadClient();

    const page = await client.audit();
    await client.audit(page.nextAfter);

    expect(page.nextAfter).toBe('8');
    expect(fetchMock).toHaveBeenLastCalledWith('/_control/v1/platform/audit?after=8', expect.any(Object));
  });

  it('production break: rejects an unsafe numeric audit cursor instead of corrupting it', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({
      ...auditPage, next_after: Number.MAX_SAFE_INTEGER + 2,
    })));

    await expect(new ProductionControlReadClient().audit()).rejects.toMatchObject({
      status: 200, message: 'Audit log is unavailable.',
    });
  });

  it('production break: rejects negative and fractional wire revisions', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({
      ...auditPage, revision: -1, items: [{ ...auditPage.items[0], revision: 8.5 }],
    })));

    await expect(new ProductionControlReadClient().audit()).rejects.toMatchObject({
      status: 200, message: 'Audit log is unavailable.',
    });
  });

  it('production break: rejects an oversized opaque cursor before making a request', async () => {
    const fetchMock = vi.fn().mockResolvedValue(json(tenantPage));
    vi.stubGlobal('fetch', fetchMock);
    const oversizedCursor = 'x'.repeat(513);

    await expect(new ProductionControlReadClient().tenants(oversizedCursor)).rejects.toMatchObject({
      status: 400, message: 'Tenant list is unavailable.',
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('production break: turns an RFC 9457 unauthorized response into a sanitized sign-in state', async () => {
    const responseSecret = 'upstream-token';
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({
      type: 'https://example.test/problems/not-authenticated', title: 'Unauthorized', status: 401,
      detail: `token ${responseSecret} was rejected`, instance: '/_control/v1/overview?ticket=private',
    }, 401, { 'content-type': 'application/problem+json' })));

    await expect(new ProductionControlReadClient().overview()).rejects.toBeInstanceOf(ControlSignInRequiredError);
    await expect(new ProductionControlReadClient().overview()).rejects.toMatchObject({
      status: 401, message: 'Sign in is required to access the control service.',
    });
  });

  it('production break: turns a forbidden response into a sanitized forbidden state', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({ detail: 'private role data' }, 403, {
      'content-type': 'application/problem+json',
    })));

    await expect(new ProductionControlReadClient().overview()).rejects.toBeInstanceOf(ControlForbiddenError);
  });

  it('production break: treats a bare not found response as an absent control scope', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 404 })));

    await expect(new ProductionControlReadClient().tenants()).rejects.toBeInstanceOf(ControlScopeAbsentError);
  });

  it('production break: turns a named RFC 9457 conflict into a sanitized conflict state', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({
      type: 'about:blank', title: 'Conflict', status: 409, code: 'ControlRevisionConflict',
      detail: 'revision secret-9 conflicts',
    }, 409, { 'content-type': 'application/problem+json' })));

    const error = await new ProductionControlReadClient().effectiveSettings().catch((value: unknown) => value);
    expect(error).toBeInstanceOf(ControlConflictError);
    expect(error).toMatchObject({
      status: 409, message: 'The control resource changed. Refresh and try again.',
    });
  });

  it('production break: keeps an unrelated RFC 9457 conflict generic', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({
      type: 'about:blank', title: 'Conflict', status: 409, code: 'ControlIdempotencyConflict',
      detail: 'idempotency-secret',
    }, 409, { 'content-type': 'application/problem+json' })));

    await expect(new ProductionControlReadClient().effectiveSettings()).rejects.toMatchObject({
      status: 409, message: 'Effective settings is unavailable.', name: 'ControlApiError',
    });
  });

  it('production break: propagates an abort without wrapping it', async () => {
    const abort = new DOMException('Aborted', 'AbortError');
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(abort));

    await expect(new ProductionControlReadClient().overview(new AbortController().signal)).rejects.toBe(abort);
  });

  it('production break: keeps CSRF only in the live client session view', async () => {
    const sessionStorageGet = vi.fn();
    const localStorageGet = vi.fn();
    vi.stubGlobal('sessionStorage', { getItem: sessionStorageGet, setItem: sessionStorageGet });
    vi.stubGlobal('localStorage', { getItem: localStorageGet, setItem: localStorageGet });
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({
      authenticated: true, principal: 'operator', csrf_token: 'csrf-secret', expires_in_s: 60,
    })));

    const session = await new ProductionControlReadClient().session();

    expect(session).toMatchObject({ authenticated: true, principal: 'operator', csrfToken: 'csrf-secret' });
    expect(sessionStorageGet).not.toHaveBeenCalled();
    expect(localStorageGet).not.toHaveBeenCalled();
  });

  it('production break: does not expose response secrets in a generic error', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json({ detail: 'response-secret' }, 503, {
      'content-type': 'application/problem+json',
    })));

    await expect(new ProductionControlReadClient().overview()).rejects.toMatchObject({
      status: 503, message: 'Platform overview is unavailable.',
    });
    await expect(new ProductionControlReadClient().overview()).rejects.not.toThrow('response-secret');
    await expect(new ProductionControlReadClient().overview()).rejects.toBeInstanceOf(ControlApiError);
  });

  it('production break: sanitizes a malformed successful response before its body reaches an error', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('response-secret', { status: 200 })));

    await expect(new ProductionControlReadClient().overview()).rejects.toMatchObject({
      status: 200, message: 'Platform overview is unavailable.',
    });
    await expect(new ProductionControlReadClient().overview()).rejects.not.toThrow('response-secret');
  });
});

describe('fixture control read client', () => {
  it('production break: a complete deterministic fixture journey performs zero network requests', async () => {
    const fetchMock = vi.fn(() => { throw new Error('fixture client must not fetch'); });
    vi.stubGlobal('fetch', fetchMock);
    const client = new FixtureControlReadClient();

    const first = await Promise.all([
      client.session(), client.overview(), client.tenants(), client.effectiveSettings(), client.audit(),
    ]);
    const second = await Promise.all([
      client.session(), client.overview(), client.tenants(), client.effectiveSettings(), client.audit(),
    ]);

    expect(second).toEqual(first);
    expect(fetchMock).not.toHaveBeenCalled();
    expect(client).not.toHaveProperty('mutate');
  });
});
