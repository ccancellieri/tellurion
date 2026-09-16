/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ControlForbiddenError,
  type CatalogView,
  type CollectionView,
  type ControlPage,
  type ControlReadClient,
  type ControlSessionView,
} from '../lib/control-api';
import { TellurionControlShell, mountControlWorkspace, workspaceModeFor, workspaceScopeFor } from './control-shell';

const catalog: CatalogView = {
  controlRevision: 8, entityVersion: '3', resource: {
    id: 'cadastre', tenant: 'tenant-a', settings: {}, visibility: { public: false, sharedWith: [] }, tombstoned: false,
  },
};
const collection: CollectionView = {
  controlRevision: 8, entityVersion: '4', resource: {
    id: 'roads', catalog: 'cadastre', kind: 'vector', settings: {}, visibility: { public: false, sharedWith: [] }, tombstoned: false,
  },
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

class ScopedClient implements ControlReadClient {
  calls: string[] = [];
  catalogPages: ControlPage<CatalogView>[] = [{ controlRevision: 8, items: [catalog], nextAfter: 'cadastre' }];
  collectionPages: ControlPage<CollectionView>[] = [{ controlRevision: 8, items: [collection], nextAfter: 'roads' }];
  session(): Promise<ControlSessionView> { this.calls.push('session'); return Promise.resolve({ authenticated: true, principal: 'operator' }); }
  overview() { this.calls.push('overview'); return Promise.reject(new Error('platform read')); }
  tenants() { this.calls.push('tenants'); return Promise.reject(new Error('inventory read')); }
  effectiveSettings() { this.calls.push('effectiveSettings'); return Promise.reject(new Error('settings read')); }
  audit() { this.calls.push('audit'); return Promise.reject(new Error('audit read')); }
  catalogs(tenant: string, after?: string) {
    this.calls.push(`catalogs:${tenant}:${after ?? ''}`);
    return Promise.resolve(this.catalogPages.shift() ?? { controlRevision: 8, items: [] });
  }
  collections(tenant: string, catalogId: string, after?: string) {
    this.calls.push(`collections:${tenant}:${catalogId}:${after ?? ''}`);
    return Promise.resolve(this.collectionPages.shift() ?? { controlRevision: 8, items: [] });
  }
}

async function settle() {
  await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
}

function mount(path: string, client: ScopedClient): TellurionControlShell {
  history.replaceState({}, '', path);
  const shell = document.createElement('tellurion-control-shell') as TellurionControlShell;
  shell.client = client;
  document.body.append(shell);
  return shell;
}

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.replaceChildren();
  history.replaceState({}, '', '/ui/control');
});

describe('scoped control workspace', () => {
  it('production break: recognizes only exact canonical scoped paths', () => {
    expect(workspaceScopeFor('/ui/control/tenants/tenant-a')).toEqual({ kind: 'tenant', tenant: 'tenant-a' });
    expect(workspaceScopeFor('/ui/control/tenants/tenant-a/catalogs/cadastre')).toEqual({ kind: 'catalog', tenant: 'tenant-a', catalog: 'cadastre' });
    expect(workspaceModeFor('/ui/control/tenants/tenant-a', 'development')).toBe('production');
    expect(workspaceModeFor('/ui/control/tenants/tenant-a/catalogs/cadastre', 'public-demo')).toBe('fixture');
    for (const path of ['/ui/control/tenants/a%2Fb', '/ui/control/tenants/..', '/ui/control/tenants/x/catalogs', '/ui/control/tenants/x/catalogs/y/other', '/ui/control/tenants/' + 'x'.repeat(129)]) {
      expect(workspaceScopeFor(path)).toBeUndefined();
      expect(workspaceModeFor(path, 'development')).toBeUndefined();
    }
  });

  it('production break: tenant workspace reads only its catalog list and paginates without platform calls', async () => {
    const client = new ScopedClient();
    client.catalogPages.push({ controlRevision: 8, items: [{ ...catalog, resource: { ...catalog.resource, id: 'surveys' } }] });
    const shell = mount('/ui/control/tenants/tenant-a', client);
    await settle();

    expect(client.calls).toEqual(['session', 'catalogs:tenant-a:']);
    expect(shell.textContent).toContain('cadastre');
    expect(shell.querySelector('[data-field="platform-editor-slot"]')).toBeNull();
    expect(shell.querySelector('[data-nav="control"]')).toBeNull();
    shell.querySelector<HTMLButtonElement>('[data-action="more-scoped"]')!.click();
    await settle();
    expect(client.calls).toEqual(['session', 'catalogs:tenant-a:', 'catalogs:tenant-a:cadastre']);
    expect(shell.textContent).toContain('surveys');
  });

  it('production break: catalog workspace reads only its collection list and preserves scope in sign-in', async () => {
    const client = new ScopedClient();
    const shell = mount('/ui/control/tenants/tenant-a/catalogs/cadastre', client);
    await settle();
    expect(client.calls).toEqual(['session', 'collections:tenant-a:cadastre:']);
    expect(shell.textContent).toContain('roads');
    expect(shell.querySelector('[data-field="platform-editor-slot"]')).toBeNull();

    document.body.replaceChildren();
    const anonymous = new ScopedClient();
    anonymous.session = () => Promise.resolve({ authenticated: false });
    const signInShell = mount('/ui/control/tenants/tenant-a/catalogs/cadastre?ignored=1#also-ignored', anonymous);
    await settle();
    expect(signInShell.querySelector<HTMLAnchorElement>('[data-action="sign-in"]')?.getAttribute('href'))
      .toBe('/_auth/control/login?return_to=/ui/control/tenants/tenant-a/catalogs/cadastre');
  });

  it('production break: denial on a later page scrubs already rendered scoped data', async () => {
    const client = new ScopedClient();
    client.catalogs = (tenant, after) => {
      client.calls.push(`catalogs:${tenant}:${after ?? ''}`);
      return after ? Promise.reject(new ControlForbiddenError()) : Promise.resolve({ controlRevision: 8, items: [catalog], nextAfter: 'cadastre' });
    };
    const shell = mount('/ui/control/tenants/tenant-a', client);
    await settle();
    shell.querySelector<HTMLButtonElement>('[data-action="more-scoped"]')!.click();
    await settle();
    expect(shell.textContent).not.toContain('cadastre');
    expect(shell.querySelector('.control-sheet')).toBeNull();
  });

  it('production break: a stale scoped response cannot repopulate a later scope', async () => {
    const oldPage = deferred<ControlPage<CatalogView>>();
    const client = new ScopedClient();
    client.catalogs = (tenant) => tenant === 'tenant-a' ? oldPage.promise : Promise.resolve({ controlRevision: 8, items: [] });
    const shell = mount('/ui/control/tenants/tenant-a', client);
    await settle();
    shell.remove();
    history.replaceState({}, '', '/ui/control/tenants/tenant-b');
    document.body.append(shell);
    await settle();
    oldPage.resolve({ controlRevision: 8, items: [catalog] });
    await settle();
    expect(shell.textContent).toContain('tenant-b');
    expect(shell.textContent).not.toContain('cadastre');
  });

  it('production break: public demo scoped route remains fixture-only', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);
    history.replaceState({}, '', '/ui/control/tenants/tenant-a');
    mountControlWorkspace(document.body, 'fixture');
    await settle();
    expect(fetchMock).not.toHaveBeenCalled();
    expect(document.body.textContent).toContain('fixture-only');
  });
});
