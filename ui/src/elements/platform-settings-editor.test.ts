/** @vitest-environment happy-dom */
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProductionControlReadClient } from '../lib/control-api';
import { TellurionPlatformSettingsEditor } from './platform-settings-editor';
import { mountControlWorkspace } from './control-shell';

const session = { authenticated: true, principal: 'operator', csrf_token: 'secret', expires_in_s: 60 };
const settings = { control_revision: 7, entity_version: '5', resource: {
  cache_ttl_s: 30, opaque: { note: '<img src=x onerror=alert(1)>' },
} };
const preview = { base_revision: 7, prospective_revision: 8, changed_resources: ['platform'], entity_versions: { platform: '8' } };
const commit = { revision: 8, changed_resources: ['platform'], replayed: false };

function json(value: unknown, status = 200): Response {
  return new Response(JSON.stringify(value), { status });
}

function problem(code: string): Response {
  return new Response(JSON.stringify({ type: 'about:blank', title: 'Conflict', status: 409, code }), {
    status: 409, headers: { 'content-type': 'application/problem+json' },
  });
}

function deferred<T>(): { promise: Promise<T>; resolve(value: T): void; reject(reason: unknown): void } {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

async function settle(): Promise<void> {
  for (let i = 0; i < 12; i += 1) await Promise.resolve();
}

function mount(): TellurionPlatformSettingsEditor {
  const editor = document.createElement('tellurion-platform-settings-editor') as TellurionPlatformSettingsEditor;
  editor.client = new ProductionControlReadClient();
  document.body.append(editor);
  return editor;
}

function click(editor: HTMLElement, action: string): void {
  editor.querySelector<HTMLButtonElement>(`[data-action="${action}"]`)!.click();
}

function edit(editor: HTMLElement, value: string): void {
  const input = editor.querySelector<HTMLInputElement>('[data-field="cache-ttl"]')!;
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

describe('production platform settings editor', () => {
  it('mounts only in the admitted production shell and survives rail refresh with an uncertain write', async () => {
    const pending = deferred<Response>();
    const fetchMock = vi.fn((path: string, init: RequestInit) => {
      if (path === '/_auth/control/session') return Promise.resolve(json(session));
      if (path === '/_control/v1/platform/overview') return Promise.resolve(json({
        scope: 'self', store_revision: 7, applied_revision: 7, lag: 0,
        poll_failures: 0, activation_failures: 0, config_version: 'revision-7',
      }));
      if (path === '/_control/v1/tenants') return Promise.resolve(json({ control_revision: 7, items: [] }));
      if (path === '/_control/v1/platform/effective-settings') return Promise.resolve(json({
        applied_revision: 7, effective: { node: { level: 'platform' }, settings: {} },
      }));
      if (path === '/_control/v1/platform/audit') return Promise.resolve(json({ revision: 7, items: [{
        revision: 7, actor: { issuer: 'https://issuer.test', subject: 'operator' }, method: 'PUT',
        canonical_path: '/_control/v1/platform/settings', correlation_id: 'event-7',
        changed_resources: ['platform'], recorded_at_unix_ms: 1, applying_instance: 'instance',
      }], next_after: 7 }));
      if (path === '/_control/v1/platform/audit?after=7') return Promise.resolve(json({ revision: 7, items: [] }));
      if (path === '/_control/v1/platform/settings' && init.method === 'GET') return Promise.resolve(json(settings));
      if (path === '/_control/v1/platform/settings?dry_run=true') return Promise.resolve(json(preview));
      if (path === '/_control/v1/platform/settings' && init.method === 'PUT') return pending.promise;
      throw new Error('Unexpected path');
    });
    vi.stubGlobal('fetch', fetchMock);
    mountControlWorkspace(document.body, 'production');
    await vi.waitFor(() => expect(document.querySelector('[data-field="cache-ttl"]')).not.toBeNull());
    const editor = document.querySelector('tellurion-platform-settings-editor') as TellurionPlatformSettingsEditor;
    edit(editor, '60');
    click(editor, 'preview');
    await vi.waitFor(() => expect(editor.querySelector('[data-action="apply"]')).not.toBeNull());
    click(editor, 'apply');
    pending.reject(new TypeError('network failed'));
    await vi.waitFor(() => expect(editor.querySelector('[data-action="retry"]')).not.toBeNull());
    document.querySelector<HTMLButtonElement>('[data-action="more-audit"]')!.click();
    await settle();
    expect(document.querySelector('tellurion-platform-settings-editor')).toBe(editor);
    expect(editor.querySelector('[data-action="retry"]')).not.toBeNull();
    expect(editor.hasUnresolvedWrite()).toBe(true);
  });

  it.each([500, 401, 403])('retains the exact retry when a late overview refresh fails with %i', async (status) => {
    const lateOverview = deferred<Response>();
    const secondApply = deferred<Response>();
    let overviewCalls = 0;
    let settingsReads = 0;
    let applyCalls = 0;
    const fetchMock = vi.fn((path: string, init: RequestInit) => {
      if (path === '/_auth/control/session') return Promise.resolve(json(session));
      if (path === '/_control/v1/platform/overview') {
        overviewCalls += 1;
        return overviewCalls === 2 ? lateOverview.promise : Promise.resolve(json({
          scope: 'self', store_revision: 8, applied_revision: 7, lag: 1,
          poll_failures: 0, activation_failures: 0, config_version: 'revision-8',
        }));
      }
      if (path === '/_control/v1/tenants') return Promise.resolve(json({ control_revision: 8, items: [] }));
      if (path === '/_control/v1/platform/effective-settings') return Promise.resolve(json({
        applied_revision: 7, effective: { node: { level: 'platform' }, settings: {} },
      }));
      if (path === '/_control/v1/platform/audit') return Promise.resolve(json({ revision: 8, items: [] }));
      if (path === '/_control/v1/platform/settings' && init.method === 'GET') {
        settingsReads += 1;
        return Promise.resolve(json(settingsReads === 1 ? settings : {
          ...settings, control_revision: 8, entity_version: '8', resource: { ...settings.resource, cache_ttl_s: 60 },
        }));
      }
      if (path === '/_control/v1/platform/settings?dry_run=true') return Promise.resolve(json(preview));
      if (path === '/_control/v1/platform/settings' && init.method === 'PUT') {
        applyCalls += 1;
        if (applyCalls === 2) return secondApply.promise;
        return Promise.resolve(json({ ...commit, revision: applyCalls === 1 ? 8 : 9, replayed: applyCalls === 3 }));
      }
      throw new Error('Unexpected path');
    });
    vi.stubGlobal('fetch', fetchMock);
    mountControlWorkspace(document.body, 'production');
    await vi.waitFor(() => expect(document.querySelector('[data-field="cache-ttl"]')).not.toBeNull());
    const editor = document.querySelector('tellurion-platform-settings-editor') as TellurionPlatformSettingsEditor;
    edit(editor, '60');
    click(editor, 'preview');
    await vi.waitFor(() => expect(editor.querySelector('[data-action="apply"]')).not.toBeNull());
    click(editor, 'apply');
    await vi.waitFor(() => expect(overviewCalls).toBe(2));
    await vi.waitFor(() => expect(editor.querySelector<HTMLInputElement>('[data-field="cache-ttl"]')?.value).toBe('60'));

    edit(editor, '70');
    click(editor, 'preview');
    await vi.waitFor(() => expect(editor.querySelector('[data-action="apply"]')).not.toBeNull());
    click(editor, 'apply');
    expect(editor.hasUnresolvedWrite()).toBe(true);
    lateOverview.resolve(json({ detail: 'private refresh failure' }, status));
    await vi.waitFor(() => expect(document.querySelector('[data-field="control-refresh-status"]')?.textContent).toBeTruthy());
    expect(document.querySelector('tellurion-platform-settings-editor')).toBe(editor);
    expect(editor.hasUnresolvedWrite()).toBe(true);
    expect(document.body.textContent).not.toContain('private refresh failure');
    if (status === 401 || status === 403) {
      expect(document.querySelector('.control-sheet__ledger')).toBeNull();
      expect(document.body.textContent).toContain('pending authorization');
    }

    secondApply.reject(new TypeError('network failure'));
    await vi.waitFor(() => expect(editor.querySelector('[data-action="retry"]')).not.toBeNull());
    click(editor, 'retry');
    await vi.waitFor(() => expect(applyCalls).toBe(3));
    const writes = fetchMock.mock.calls.filter(([path, init]) => path === '/_control/v1/platform/settings' && init.method === 'PUT');
    expect(writes[1][1].body).toBe(writes[2][1].body);
  });

  it('loads only raw cache lifetime and version, keeps opaque data hidden, and requires preview before apply', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview));
    vi.stubGlobal('fetch', fetchMock);
    const editor = mount();
    await settle();

    expect(editor.textContent).toContain('Platform-wide');
    expect(editor.textContent).toContain('inherited defaults');
    expect(editor.textContent).toContain('Entity version 5');
    expect(editor.querySelector<HTMLInputElement>('[data-field="cache-ttl"]')?.value).toBe('30');
    expect(editor.innerHTML).not.toContain('onerror');
    expect(editor.innerHTML).not.toContain('secret');
    expect(editor.querySelector('[data-action="apply"]')).toBeNull();

    edit(editor, '60');
    click(editor, 'preview');
    await settle();
    expect(editor.textContent).toContain('Revision 7 → 8');
    expect(editor.textContent).toContain('platform');
    expect(editor.querySelector('[data-action="apply"]')).not.toBeNull();
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });

  it('keeps the raw editor noninteractive during its read and escapes the entity version', async () => {
    const pending = deferred<Response>();
    const fetchMock = vi.fn().mockResolvedValueOnce(json(session)).mockImplementationOnce(() => pending.promise);
    vi.stubGlobal('fetch', fetchMock);
    const editor = mount();
    await settle();
    expect(editor.querySelector('[aria-busy="true"]')).not.toBeNull();
    expect(editor.querySelector('[data-field="cache-ttl"]')).toBeNull();
    expect(editor.querySelector('[data-action="preview"]')).toBeNull();
    pending.resolve(json({ ...settings, entity_version: '<img src=x onerror=alert(1)>' }));
    await vi.waitFor(() => expect(editor.querySelector('[data-field="cache-ttl"]')).not.toBeNull());
    expect(editor.querySelector('img')).toBeNull();
    expect(editor.textContent).toContain('<img src=x onerror=alert(1)>');
    expect(editor.innerHTML).not.toContain('<img src=x');
  });

  it('never mounts a production editor for an unauthenticated or forbidden shell', async () => {
    const unauthenticated = vi.fn().mockResolvedValue(json({ authenticated: false }));
    vi.stubGlobal('fetch', unauthenticated);
    mountControlWorkspace(document.body, 'production');
    await vi.waitFor(() => expect(document.body.textContent).toContain('Sign in to control Tellurion'));
    expect(document.querySelector('tellurion-platform-settings-editor')).toBeNull();
    expect(unauthenticated).toHaveBeenCalledTimes(1);

    document.body.replaceChildren();
    const forbidden = vi.fn((path: string) => path === '/_auth/control/session'
      ? Promise.resolve(json(session)) : Promise.resolve(json({ detail: 'private' }, 403)));
    vi.stubGlobal('fetch', forbidden);
    mountControlWorkspace(document.body, 'production');
    await vi.waitFor(() => expect(document.body.textContent).toContain('Platform scope unavailable'));
    expect(document.querySelector('tellurion-platform-settings-editor')).toBeNull();
    expect(forbidden.mock.calls.map(([path]) => path)).not.toContain('/_control/v1/platform/settings');
  });

  it('editing invalidates a preview without rerendering the focused input', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview)));
    const editor = mount();
    await settle();
    click(editor, 'preview');
    await settle();
    const input = editor.querySelector<HTMLInputElement>('[data-field="cache-ttl"]')!;
    input.focus();
    edit(editor, '61');

    expect(editor.querySelector('[data-field="cache-ttl"]')).toBe(input);
    expect(document.activeElement).toBe(input);
    expect(editor.querySelector('[data-action="apply"]')).toBeNull();
    expect(editor.textContent).not.toContain('Revision 7 → 8');
  });

  it('validates a nonnegative integer and supports explicit unset', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview));
    vi.stubGlobal('fetch', fetchMock);
    const editor = mount();
    await settle();
    edit(editor, '-1');
    click(editor, 'preview');
    expect(editor.textContent).toContain('non-negative whole');
    expect(fetchMock).toHaveBeenCalledTimes(2);
    edit(editor, '');
    click(editor, 'preview');
    await settle();
    expect(JSON.parse(fetchMock.mock.calls[2][1].body).operations[0].operation.SetPlatformSettings.cache_ttl_s).toBeNull();
  });

  it('retains draft on conflict, rebases against the new entity version, and previews again', async () => {
    const fetchMock = vi.fn().mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(problem('ControlEntityVersionConflict'))
      .mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json({ ...settings, entity_version: '6' }))
      .mockResolvedValueOnce(json(preview));
    vi.stubGlobal('fetch', fetchMock);
    const editor = mount();
    await settle();
    edit(editor, '60');
    click(editor, 'preview');
    await settle();
    expect(editor.querySelector<HTMLInputElement>('[data-field="cache-ttl"]')?.value).toBe('60');
    expect(editor.querySelector('[data-action="apply"]')).toBeNull();
    click(editor, 'rebase');
    await settle();
    expect(editor.querySelector<HTMLInputElement>('[data-field="cache-ttl"]')?.value).toBe('60');
    expect(editor.textContent).toContain('Entity version 6');
    click(editor, 'preview');
    await settle();
    expect(JSON.parse(fetchMock.mock.calls[5][1].body).operations[0].expected_entity_version).toBe('6');
  });

  it('holds an uncertain apply for exact retry, blocks double clicks, then reloads the source', async () => {
    const pending = deferred<Response>();
    const fetchMock = vi.fn().mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json(settings))
      .mockResolvedValueOnce(json(preview)).mockImplementationOnce(() => pending.promise)
      .mockResolvedValueOnce(json(commit)).mockResolvedValueOnce(json(session))
      .mockResolvedValueOnce(json({ ...settings, control_revision: 8, entity_version: '8' }));
    vi.stubGlobal('fetch', fetchMock);
    const editor = mount();
    await settle();
    edit(editor, '60');
    click(editor, 'preview');
    await settle();
    const apply = editor.querySelector<HTMLButtonElement>('[data-action="apply"]')!;
    apply.click();
    apply.click();
    expect(fetchMock).toHaveBeenCalledTimes(4);
    expect(editor.hasUnresolvedWrite()).toBe(true);
    const leaving = new Event('beforeunload', { cancelable: true });
    window.dispatchEvent(leaving);
    expect(leaving.defaultPrevented).toBe(true);
    pending.reject(new TypeError('network failed'));
    await settle();
    expect(editor.querySelector('[data-action="apply"]')).toBeNull();
    expect(editor.querySelector('[data-action="preview"]')).toBeNull();
    click(editor, 'retry');
    await vi.waitFor(() => expect(editor.textContent).toContain('Entity version 8'));
    expect(fetchMock.mock.calls[4][1].body).toBe(fetchMock.mock.calls[3][1].body);
    expect(editor.textContent).toContain('revision 8');
    expect(editor.textContent).toContain('Entity version 8');
    expect(editor.hasUnresolvedWrite()).toBe(false);
  });

  it('does not let an old pending result render after disconnect and replacement', async () => {
    const pending = deferred<Response>();
    const fetchMock = vi.fn().mockResolvedValueOnce(json(session)).mockImplementationOnce(() => pending.promise)
      .mockResolvedValueOnce(json(session)).mockResolvedValueOnce(json(settings));
    vi.stubGlobal('fetch', fetchMock);
    const old = mount();
    await settle();
    old.remove();
    const current = mount();
    await settle();
    pending.resolve(json({ ...settings, entity_version: 'stale' }));
    await settle();
    expect(current.textContent).toContain('Entity version 5');
    expect(current.textContent).not.toContain('stale');
  });
});
