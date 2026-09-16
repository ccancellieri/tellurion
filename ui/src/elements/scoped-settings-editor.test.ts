/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { mountControlWorkspace } from './control-shell';
import { TellurionPlatformSettingsEditor } from './platform-settings-editor';

const session = { authenticated: true, principal: 'operator', csrf_token: 'secret', expires_in_s: 60 };
const list = { control_revision: 7, items: [{ control_revision: 7, entity_version: '5', resource: {
  id: 'cadastre', tenant: 'tenant-a', settings: {}, visibility: { public: false, shared_with: [] }, tombstoned: false,
} }], next_after: 'cadastre' };
const settings = { control_revision: 7, entity_version: '5', resource: { cache_ttl_s: 30, opaque: { retained: true } } };
const key = 'tenant/tenant-a';

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), { status });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.replaceChildren();
  history.replaceState({}, '', '/ui/control');
});

describe('scoped production settings editor', () => {
  it('mounts a catalog editor without reading tenant or platform settings', async () => {
    history.replaceState({}, '', '/ui/control/tenants/tenant-a/catalogs/cadastre');
    const fetchMock = vi.fn((path: string) => {
      if (path === '/_auth/control/session') return Promise.resolve(json(session));
      if (path === '/_control/v1/tenants/tenant-a/catalogs/cadastre/collections') return Promise.resolve(json({ control_revision: 7, items: [] }));
      if (path === '/_control/v1/tenants/tenant-a/catalogs/cadastre/settings') return Promise.resolve(json(settings));
      throw new Error(`Unexpected endpoint ${path}`);
    });
    vi.stubGlobal('fetch', fetchMock);
    mountControlWorkspace(document.body, 'production');
    await vi.waitFor(() => expect(document.querySelector('[data-field="cache-ttl"]')).not.toBeNull());
    expect(document.querySelector('tellurion-platform-settings-editor')?.textContent).toContain('Catalog settings');
    expect(fetchMock.mock.calls.map(([path]) => path)).toEqual([
      '/_auth/control/session',
      '/_control/v1/tenants/tenant-a/catalogs/cadastre/collections',
      '/_auth/control/session',
      '/_control/v1/tenants/tenant-a/catalogs/cadastre/settings',
    ]);
  });

  it('mounts for a tenant and preserves an unresolved apply through pagination and denial', async () => {
    history.replaceState({}, '', '/ui/control/tenants/tenant-a');
    const pendingApply = deferred<Response>();
    const fetchMock = vi.fn((path: string, init: RequestInit) => {
      if (path === '/_auth/control/session') return Promise.resolve(json(session));
      if (path === '/_control/v1/tenants/tenant-a/catalogs') return Promise.resolve(json(list));
      if (path === '/_control/v1/tenants/tenant-a/catalogs?after=cadastre') return Promise.resolve(json({
        control_revision: 7,
        items: [{ ...list.items[0], resource: { ...list.items[0].resource, id: 'surveys' } }],
        next_after: 'surveys',
      }));
      if (path === '/_control/v1/tenants/tenant-a/catalogs?after=surveys') return Promise.resolve(json({ detail: 'private' }, 403));
      if (path === '/_control/v1/tenants/tenant-a/settings' && init.method === 'GET') return Promise.resolve(json(settings));
      if (path === '/_control/v1/tenants/tenant-a/settings?dry_run=true') return Promise.resolve(json({
        base_revision: 7, prospective_revision: 8, changed_resources: [key], entity_versions: { [key]: '8' },
      }));
      if (path === '/_control/v1/tenants/tenant-a/settings' && init.method === 'PUT') return pendingApply.promise;
      throw new Error(`Unexpected endpoint ${path}`);
    });
    vi.stubGlobal('fetch', fetchMock);
    mountControlWorkspace(document.body, 'production');
    await vi.waitFor(() => expect(document.querySelector('[data-field="cache-ttl"]')).not.toBeNull());
    const editor = document.querySelector('tellurion-platform-settings-editor') as TellurionPlatformSettingsEditor;
    expect(editor.textContent).toContain('Tenant settings');
    const input = editor.querySelector<HTMLInputElement>('[data-field="cache-ttl"]')!;
    input.value = '60';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    editor.querySelector<HTMLButtonElement>('[data-action="preview"]')!.click();
    await vi.waitFor(() => expect(editor.querySelector('[data-action="apply"]')).not.toBeNull());
    editor.querySelector<HTMLButtonElement>('[data-action="apply"]')!.click();
    expect(editor.hasUnresolvedWrite()).toBe(true);
    document.querySelector<HTMLButtonElement>('[data-action="more-scoped"]')!.click();
    await vi.waitFor(() => expect(document.body.textContent).toContain('surveys'));
    expect(document.querySelector('tellurion-platform-settings-editor')).toBe(editor);
    document.querySelector<HTMLButtonElement>('[data-action="more-scoped"]')!.click();
    await vi.waitFor(() => expect(document.body.textContent).toContain('pending authorization'));
    expect(document.body.textContent).not.toContain('cadastre');
    expect(document.body.textContent).not.toContain('surveys');
    expect(document.querySelector('tellurion-platform-settings-editor')).toBe(editor);
    pendingApply.reject(new TypeError('network failed'));
    await vi.waitFor(() => expect(editor.querySelector('[data-action="retry"]')).not.toBeNull());
    expect(editor.hasUnresolvedWrite()).toBe(true);
    expect(fetchMock.mock.calls.map(([path]) => path)).not.toContain('/_control/v1/platform/settings');
    expect(fetchMock.mock.calls.map(([path]) => path)).not.toContain('/_control/v1/tenants/tenant-b/settings');
  });

  it('never mounts a mutating editor in a fixture scoped workspace', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    history.replaceState({}, '', '/ui/control/tenants/tenant-a/catalogs/cadastre');
    mountControlWorkspace(document.body, 'fixture');
    await vi.waitFor(() => expect(document.querySelector('[data-scope="catalog"]')).not.toBeNull());
    expect(document.querySelector('tellurion-platform-settings-editor')).toBeNull();
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
