/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  ControlApiError,
  ControlForbiddenError,
  type ControlAuditPage,
  type ControlOverview,
  type ControlPage,
  type ControlReadClient,
  type ControlSessionView,
  type EffectiveSettingsView,
  type TenantView,
} from '../lib/control-api';
import { controlFixtures } from '../lib/control-fixtures';
import { TellurionControlShell, mountControlWorkspace, workspaceModeFor } from './control-shell';

const session: ControlSessionView = { authenticated: true, principal: 'operator' };
const overview: ControlOverview = {
  scope: 'self', storeRevision: 8, appliedRevision: 7, lag: 1,
  pollFailures: 0, activationFailures: 0, configVersion: 'revision-7',
};
const tenantA: TenantView = {
  controlRevision: 8, entityVersion: '3', resource: { id: 'tenant-a', settings: {}, tombstoned: false },
};
const tenantB: TenantView = {
  controlRevision: 8, entityVersion: '4', resource: { id: 'tenant-b', settings: {}, tombstoned: false },
};
const settings: EffectiveSettingsView = {
  appliedRevision: 7,
  effective: {
    node: { level: 'platform' },
    settings: { cache_ttl_s: { value: 30, provenance: { kind: 'local_override' } } },
  },
};
const auditA: ControlAuditPage = {
  revision: 8,
  items: [{
    revision: 8, actor: { issuer: 'https://issuer.example', subject: 'operator' }, method: 'PUT',
    canonicalPath: '/_control/v1/platform/settings', correlationId: 'correlation-a',
    changedResources: ['platform'], recordedAtUnixMs: 1_700_000_000_000, applyingInstance: 'instance-a',
  }],
  nextAfter: '8',
};
const auditB: ControlAuditPage = {
  revision: 9,
  items: [{
    revision: 9, actor: { issuer: 'https://issuer.example', subject: 'operator' }, method: 'PUT',
    canonicalPath: '/_control/v1/tenants/tenant-b', correlationId: 'correlation-b',
    changedResources: ['tenant-b'], recordedAtUnixMs: 1_700_000_010_000, applyingInstance: 'instance-a',
  }],
};

class StubClient implements ControlReadClient {
  sessionValue: ControlSessionView | Promise<ControlSessionView> = session;
  overviewValue: ControlOverview | Promise<ControlOverview> = overview;
  tenantPages: ControlPage<TenantView>[] = [{ controlRevision: 8, items: [tenantA], nextAfter: 'tenant-a' }];
  settingsValue: EffectiveSettingsView | Promise<EffectiveSettingsView> = settings;
  auditPages: ControlAuditPage[] = [auditA];
  tenantCalls: Array<string | undefined> = [];
  auditCalls: Array<string | undefined> = [];

  session(): Promise<ControlSessionView> { return Promise.resolve(this.sessionValue); }
  overview(): Promise<ControlOverview> { return Promise.resolve(this.overviewValue); }
  tenants(after?: string): Promise<ControlPage<TenantView>> {
    this.tenantCalls.push(after);
    return Promise.resolve(this.tenantPages[this.tenantCalls.length - 1] ?? { controlRevision: 8, items: [] });
  }
  effectiveSettings(): Promise<EffectiveSettingsView> { return Promise.resolve(this.settingsValue); }
  audit(after?: string): Promise<ControlAuditPage> {
    this.auditCalls.push(after);
    return Promise.resolve(this.auditPages[this.auditCalls.length - 1] ?? { revision: 8, items: [] });
  }
}

function deferred<T>(): { promise: Promise<T>; resolve(value: T): void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

async function settle(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

function mount(client: StubClient, mode: 'production' | 'fixture' = 'production'): TellurionControlShell {
  const shell = document.createElement('tellurion-control-shell') as TellurionControlShell;
  shell.client = client;
  shell.mode = mode;
  document.body.append(shell);
  return shell;
}

afterEach(() => {
  vi.restoreAllMocks();
  document.body.replaceChildren();
  history.replaceState({}, '', '/ui/control');
});

describe('control workspace', () => {
  it('production break: keeps the worksheet absent while the browser session is still loading', async () => {
    const client = new StubClient();
    const pending = deferred<ControlSessionView>();
    client.sessionValue = pending.promise;
    const shell = mount(client);

    expect(shell.querySelector('.control-sheet')).toBeNull();
    expect(shell.textContent).toContain('Loading control session');

    pending.resolve(session);
    await settle();
    expect(shell.querySelector('.control-sheet')).not.toBeNull();
  });

  it('production break: keeps authenticated panel loading noninteractive until initial reads finish', async () => {
    const client = new StubClient();
    const pendingOverview = deferred<ControlOverview>();
    client.overviewValue = pendingOverview.promise;
    const shell = mount(client);
    await settle();

    expect(shell.querySelector('.control-workspace')).toBeNull();
    expect(shell.querySelector('a, button, input, select, textarea')).toBeNull();
    expect(shell.textContent).toContain('Loading control data');

    pendingOverview.resolve(overview);
    await settle();
    expect(shell.querySelector('.control-workspace')).not.toBeNull();
    expect(shell.querySelector('[data-nav="control"]')).not.toBeNull();
  });

  it('production break: offers sign-in without rendering protected workspace data', async () => {
    const client = new StubClient();
    client.sessionValue = { authenticated: false };
    const shell = mount(client);
    await settle();

    expect(shell.querySelector('.control-sheet')).toBeNull();
    expect(shell.textContent).toContain('Sign in to control Tellurion');
    expect(shell.querySelector<HTMLAnchorElement>('[data-action="sign-in"]')?.getAttribute('href'))
      .toBe('/_auth/control/login?return_to=/ui/control');
  });

  it('production break: isolates a forbidden platform scope instead of showing data', async () => {
    const client = new StubClient();
    client.overviewValue = Promise.reject(new ControlForbiddenError());
    const shell = mount(client);
    await settle();

    expect(shell.textContent).toContain('Platform scope unavailable');
    expect(shell.querySelector('.control-sheet')).toBeNull();
  });

  it('production break: renders an empty tenant state as a scoped operational result', async () => {
    const client = new StubClient();
    client.tenantPages = [{ controlRevision: 8, items: [] }];
    const shell = mount(client);
    await settle();

    expect(shell.textContent).toContain('No tenants are available in this inventory.');
    expect(shell.querySelector('[data-field="tenant-list"]')).not.toBeNull();
  });

  it('production break: retains the overview when an independent panel is unavailable', async () => {
    const client = new StubClient();
    client.settingsValue = Promise.reject(new ControlApiError(503, 'Effective settings is unavailable.'));
    const shell = mount(client);
    await settle();

    expect(shell.textContent).toContain('/platform · revision 8 · applied 7 · lagging');
    expect(shell.textContent).toContain('Effective settings is unavailable.');
  });

  it('production break: renders the operational ledger and revision coordinate for an authenticated overview', async () => {
    const shell = mount(new StubClient());
    await settle();

    expect(shell.querySelector('.control-sheet__ledger')).not.toBeNull();
    expect(shell.textContent).toContain('/platform · revision 8 · applied 7 · lagging');
    expect(shell.textContent).toContain('Configuration version');
    expect(shell.querySelector('.control-kpi')).toBeNull();
  });

  it('production break: follows the tenant keyset continuation supplied by the service', async () => {
    const client = new StubClient();
    client.tenantPages = [
      { controlRevision: 8, items: [tenantA], nextAfter: 'tenant-a' },
      { controlRevision: 8, items: [tenantB] },
    ];
    client.auditPages = [{ revision: 8, items: [] }];
    const shell = mount(client);
    await settle();
    shell.querySelector<HTMLButtonElement>('[data-action="more-tenants"]')!.click();
    await settle();

    expect(client.tenantCalls).toEqual([undefined, 'tenant-a']);
    expect(shell.textContent).toContain('tenant-b');
    expect(document.activeElement).toBe(shell.querySelector('[data-field="tenant-list"]'));
  });

  it('production break: restores tenant inventory focus after a continuation error', async () => {
    const client = new StubClient();
    client.auditPages = [{ revision: 8, items: [] }];
    client.tenants = (after?: string) => {
      client.tenantCalls.push(after);
      return after === undefined
        ? Promise.resolve({ controlRevision: 8, items: [tenantA], nextAfter: 'tenant-a' })
        : Promise.reject(new ControlApiError(503, 'Tenant list is unavailable.'));
    };
    const shell = mount(client);
    await settle();
    shell.querySelector<HTMLButtonElement>('[data-action="more-tenants"]')!.click();
    await settle();

    expect(shell.textContent).toContain('Tenant list is unavailable.');
    expect(document.activeElement).toBe(shell.querySelector('[data-field="tenant-list"]'));
  });

  it('production break: renders typed setting provenance from the server view', async () => {
    const client = new StubClient();
    client.settingsValue = {
      appliedRevision: 7,
      effective: {
        node: { level: 'platform' },
        settings: {
          cache_ttl_s: { value: 30, provenance: { kind: 'local_override' } },
          slow_request_ms: { value: 1000, provenance: { kind: 'built_in_default' } },
          tile_caps: { value: { min_zoom: 0 }, provenance: { kind: 'derived' } },
          stac: { value: null, provenance: { kind: 'inherited', level: 'platform' } },
          protocols: { value: null, provenance: { kind: 'profile', level: 'tenant', profileId: 'fast' } },
        },
      },
    };
    const shell = mount(client);
    await settle();

    expect(shell.textContent).toContain('Effective at applied revision 7');
    expect(shell.textContent).toContain('cache_ttl_s');
    expect(shell.textContent).toContain('30');
    expect(shell.textContent).toContain('Local override');
    expect(shell.textContent).toContain('Built-in default');
    expect(shell.textContent).toContain('Derived');
    expect(shell.textContent).toContain('Inherited from platform');
    expect(shell.textContent).toContain('Profile fast at tenant');
  });

  it('production break: follows audit continuation without replacing earlier events', async () => {
    const client = new StubClient();
    client.auditPages = [auditA, auditB];
    const shell = mount(client);
    await settle();
    shell.querySelector<HTMLButtonElement>('[data-action="more-audit"]')!.click();
    await settle();

    expect(client.auditCalls).toEqual([undefined, '8']);
    expect(shell.textContent).toContain('correlation-a');
    expect(shell.textContent).toContain('correlation-b');
  });

  it('production break: preserves query and fragment context in peer workspace navigation', async () => {
    history.replaceState({}, '', '/ui/control?tenant=tenant-a&panel=settings#effective');
    const shell = mount(new StubClient());
    await settle();

    expect(shell.querySelector<HTMLAnchorElement>('[data-nav="catalog-map"]')?.getAttribute('href'))
      .toBe('/ui/?tenant=tenant-a&panel=settings#effective');
    expect(shell.querySelector<HTMLAnchorElement>('[data-nav="control"]')?.getAttribute('href'))
      .toBe('/ui/control?tenant=tenant-a&panel=settings#effective');
  });

  it('production break: keeps tenants as read-only inventory and moves keyboard focus after selecting platform scope', async () => {
    const shell = mount(new StubClient());
    await settle();
    expect(shell.querySelector('[data-scope="tenant-a"]')).toBeNull();
    shell.querySelector<HTMLButtonElement>('[data-scope="platform"]')!.click();

    expect(document.activeElement).toBe(shell.querySelector('#control-workspace-heading'));
    expect(shell.textContent).toContain('Platform control ledger');
  });

  it('production break: routes public-demo control paths to fixtures before production client creation', () => {
    expect(workspaceModeFor('/ui/control', 'public-demo')).toBe('fixture');
    expect(workspaceModeFor('/ui/control-demo', 'public-demo')).toBe('fixture');
    expect(workspaceModeFor('/ui/control', 'development')).toBe('production');
  });

  it('production break: mounts fixture data with an explicit demonstration boundary and fixture control navigation', async () => {
    history.replaceState({}, '', '/ui/control-demo?tenant=fixture-tenant#effective');
    mountControlWorkspace(document.body, 'fixture');
    await settle();

    expect(document.body.textContent).toContain('Demonstration data');
    expect(document.body.textContent).not.toContain('Apply changes');
    expect(document.querySelector<HTMLAnchorElement>('[data-nav="control"]')?.getAttribute('href'))
      .toBe('/ui/control-demo?tenant=fixture-tenant#effective');
  });

  it('fixture preview: simulates one platform setting without changing immutable fixtures or making a request', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    mountControlWorkspace(document.body, 'fixture');
    await settle();

    const input = document.querySelector<HTMLInputElement>('[data-field="fixture-cache-ttl"]')!;
    input.value = '600';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    document.querySelector<HTMLButtonElement>('[data-action="simulate-change"]')!.click();
    await settle();

    expect(document.body.textContent).toContain('300 → 600 seconds');
    expect(document.body.textContent).toContain('No control revision, audit event, or runtime setting is changed.');
    expect(document.body.textContent).toContain('would use a 600-second lifetime');
    expect(fetchSpy).not.toHaveBeenCalled();
    expect(controlFixtures.settings.effective.settings.cache_ttl_s.value).toBe(300);

    const second = document.querySelector<HTMLInputElement>('[data-field="fixture-cache-ttl"]')!;
    expect(second.value).toBe('600');
  });

  it('fixture preview: reset restores the initial draft and clears the simulation without a request', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    mountControlWorkspace(document.body, 'fixture');
    await settle();

    const input = document.querySelector<HTMLInputElement>('[data-field="fixture-cache-ttl"]')!;
    input.value = '900';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    document.querySelector<HTMLButtonElement>('[data-action="simulate-change"]')!.click();
    await settle();
    document.querySelector<HTMLButtonElement>('[data-action="reset-fixture-draft"]')!.click();
    await settle();

    expect(document.querySelector<HTMLInputElement>('[data-field="fixture-cache-ttl"]')?.value).toBe('300');
    expect(document.querySelector('[data-field="fixture-simulation"]')).toBeNull();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it('fixture preview: clears a stale simulation and exposes an error as soon as the draft becomes invalid', async () => {
    mountControlWorkspace(document.body, 'fixture');
    await settle();

    const input = document.querySelector<HTMLInputElement>('[data-field="fixture-cache-ttl"]')!;
    input.value = '600';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    document.querySelector<HTMLButtonElement>('[data-action="simulate-change"]')!.click();
    await settle();
    expect(document.querySelector('[data-field="fixture-simulation"]')).not.toBeNull();

    const invalid = document.querySelector<HTMLInputElement>('[data-field="fixture-cache-ttl"]')!;
    invalid.value = '0';
    invalid.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();

    expect(document.querySelector('[data-field="fixture-simulation"]')).toBeNull();
    expect(document.body.textContent).not.toContain('300 → 600 seconds');
    expect(document.body.textContent).not.toContain('would use a 600-second lifetime');
    expect(document.querySelector('[data-field="fixture-validation"]')?.textContent)
      .toContain('Enter a whole cache lifetime from 1 to 86400 seconds.');
  });

  it('fixture preview: restores action focus and announces simulation and reset results', async () => {
    mountControlWorkspace(document.body, 'fixture');
    await settle();

    const input = document.querySelector<HTMLInputElement>('[data-field="fixture-cache-ttl"]')!;
    input.value = '600';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    document.querySelector<HTMLButtonElement>('[data-action="simulate-change"]')!.click();
    await settle();

    expect(document.activeElement).toBe(document.querySelector('[data-action="simulate-change"]'));
    expect(document.querySelector('[data-field="fixture-preview-status"]')?.textContent)
      .toContain('Simulation complete');

    document.querySelector<HTMLButtonElement>('[data-action="reset-fixture-draft"]')!.click();
    await settle();

    expect(document.activeElement).toBe(document.querySelector('[data-action="reset-fixture-draft"]'));
    expect(document.querySelector('[data-field="fixture-preview-status"]')?.textContent)
      .toContain('Draft reset to the 300-second fixture baseline.');
  });

  it('production break: never exposes the fixture simulator or an apply action', async () => {
    const shell = mount(new StubClient(), 'production');
    await settle();

    expect(shell.querySelector('[data-field="fixture-cache-ttl"]')).toBeNull();
    expect(shell.querySelector('[data-action="simulate-change"]')).toBeNull();
    expect(shell.textContent).not.toContain('Apply changes');
  });

  it('production break: clears prior principal data before a reconnected session resolves', async () => {
    const first = new StubClient();
    const shell = mount(first);
    await settle();
    expect(shell.textContent).toContain('tenant-a');
    expect(shell.textContent).toContain('correlation-a');

    const second = new StubClient();
    second.tenantPages = [{ controlRevision: 9, items: [tenantB] }];
    second.auditPages = [auditB];
    const pending = deferred<ControlSessionView>();
    second.sessionValue = pending.promise;
    shell.remove();
    shell.client = second;
    document.body.append(shell);

    expect(shell.querySelector('.control-sheet')).toBeNull();
    expect(shell.textContent).not.toContain('tenant-a');
    expect(shell.textContent).not.toContain('correlation-a');

    pending.resolve({ authenticated: true, principal: 'new-operator' });
    await settle();
    expect(shell.textContent).toContain('tenant-b');
    expect(shell.textContent).toContain('correlation-b');
    expect(shell.textContent).not.toContain('tenant-a');
  });

  it('production break: ignores a tenant continuation that resolves after reconnection with another client', async () => {
    const first = new StubClient();
    const pending = deferred<ControlPage<TenantView>>();
    first.tenants = (after?: string) => {
      first.tenantCalls.push(after);
      return after === undefined
        ? Promise.resolve({ controlRevision: 8, items: [tenantA], nextAfter: 'tenant-a' })
        : pending.promise;
    };
    const shell = mount(first);
    await settle();
    shell.querySelector<HTMLButtonElement>('[data-action="more-tenants"]')!.click();
    shell.remove();
    const second = new StubClient();
    second.tenantPages = [{ controlRevision: 9, items: [tenantB] }];
    shell.client = second;
    document.body.append(shell);
    await settle();
    pending.resolve({ controlRevision: 8, items: [{ ...tenantA, resource: { ...tenantA.resource, id: 'stale-tenant' } }] });
    await settle();

    expect(shell.textContent).toContain('tenant-b');
    expect(shell.textContent).not.toContain('stale-tenant');
  });

  it('production break: prevents duplicate pending continuations and restores list focus after loading', async () => {
    const client = new StubClient();
    const tenantPage = deferred<ControlPage<TenantView>>();
    const auditPage = deferred<ControlAuditPage>();
    client.tenants = (after?: string) => {
      client.tenantCalls.push(after);
      return after === undefined
        ? Promise.resolve({ controlRevision: 8, items: [tenantA], nextAfter: 'tenant-a' })
        : tenantPage.promise;
    };
    client.audit = (after?: string) => {
      client.auditCalls.push(after);
      return after === undefined ? Promise.resolve(auditA) : auditPage.promise;
    };
    const shell = mount(client);
    await settle();
    const moreTenants = shell.querySelector<HTMLButtonElement>('[data-action="more-tenants"]')!;
    const moreAudit = shell.querySelector<HTMLButtonElement>('[data-action="more-audit"]')!;
    moreTenants.click();
    moreTenants.click();
    moreAudit.click();
    moreAudit.click();

    expect(client.tenantCalls).toEqual([undefined, 'tenant-a']);
    expect(client.auditCalls).toEqual([undefined, '8']);
    expect(shell.textContent).toContain('Loading tenant inventory');
    expect(shell.textContent).toContain('Loading audit events');

    tenantPage.resolve({ controlRevision: 8, items: [tenantB] });
    auditPage.resolve(auditB);
    await settle();
    expect(document.activeElement).toBe(shell.querySelector('[data-field="audit-list"]'));
    expect(shell.textContent).toContain('correlation-b');
  });
});
