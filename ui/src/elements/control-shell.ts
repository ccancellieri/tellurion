import '../control-workspace.css';
import {
  ControlApiError,
  ControlForbiddenError,
  ControlSignInRequiredError,
  ProductionControlReadClient,
  validControlScopeId,
  type CatalogView,
  type CollectionView,
  type ControlAuditItem,
  type ControlAuditPage,
  type ControlOverview,
  type ControlPage,
  type ControlReadClient,
  type EffectiveSettingsView,
  type TenantView,
} from '../lib/control-api';
import {
  FixtureControlReadClient,
  fixturePlatformSetting,
  simulateFixturePlatformSetting,
  type FixtureSettingSimulation,
} from '../lib/control-fixtures';
import { TellurionPlatformSettingsEditor } from './platform-settings-editor';

export type ControlMode = 'production' | 'fixture';
export type ControlWorkspaceScope =
  | { kind: 'platform' }
  | { kind: 'tenant'; tenant: string }
  | { kind: 'catalog'; tenant: string; catalog: string };

function escape(value: unknown): string {
  return String(value)
    .replaceAll('&', '&amp;')
    .replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;')
    .replaceAll('"', '&quot;')
    .replaceAll("'", '&#39;');
}

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof ControlApiError ? error.message : fallback;
}

function fixtureValidationMessage(error: unknown): string {
  return error instanceof Error && error.message ? error.message : 'Enter a valid cache lifetime.';
}

function settingValue(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value) ?? 'null';
}

function provenanceLabel(provenance: EffectiveSettingsView['effective']['settings'][string]['provenance']): string {
  switch (provenance.kind) {
    case 'built_in_default': return 'Built-in default';
    case 'derived': return 'Derived';
    case 'local_override': return 'Local override';
    case 'inherited': return `Inherited from ${provenance.level}`;
    case 'profile': return `Profile ${provenance.profileId} at ${provenance.level}`;
  }
}

export function workspaceScopeFor(pathname: string): ControlWorkspaceScope | undefined {
  if (pathname === '/ui/control' || pathname === '/ui/control/') return { kind: 'platform' };
  const segments = pathname.split('/');
  if (segments.length === 5 && segments[0] === '' && segments[1] === 'ui' &&
    segments[2] === 'control' && segments[3] === 'tenants' && validControlScopeId(segments[4])) {
    return { kind: 'tenant', tenant: segments[4] };
  }
  if (segments.length === 7 && segments[0] === '' && segments[1] === 'ui' &&
    segments[2] === 'control' && segments[3] === 'tenants' && validControlScopeId(segments[4]) &&
    segments[5] === 'catalogs' && validControlScopeId(segments[6])) {
    return { kind: 'catalog', tenant: segments[4], catalog: segments[6] };
  }
  return undefined;
}

export function workspaceModeFor(pathname: string, buildMode: string): ControlMode | undefined {
  const control = workspaceScopeFor(pathname) !== undefined;
  const demo = pathname === '/ui/control-demo' || pathname === '/ui/control-demo/';
  if (buildMode === 'public-demo' && (control || demo)) return 'fixture';
  if (demo) return 'fixture';
  return control ? 'production' : undefined;
}

/** A cookie-authenticated platform control workspace. */
export class TellurionControlShell extends HTMLElement {
  client!: ControlReadClient;
  mode: ControlMode = 'production';
  #overview?: ControlOverview;
  #overviewRefreshError?: string;
  #tenants: TenantView[] = [];
  #tenantAfter?: string;
  #tenantError?: string;
  #settings?: EffectiveSettingsView;
  #settingsError?: string;
  #audit: ControlAuditItem[] = [];
  #auditAfter?: string;
  #auditError?: string;
  #tenantLoading = false;
  #auditLoading = false;
  #fixtureDraft = String(fixturePlatformSetting.initialValue);
  #fixtureSimulation?: FixtureSettingSimulation;
  #fixtureValidationError?: string;
  #fixtureStatus?: string;
  #editor?: TellurionPlatformSettingsEditor;
  #generation = 0;
  #scope: ControlWorkspaceScope = { kind: 'platform' };
  #scopePath = '/ui/control';
  #scopedItems: CatalogView[] | CollectionView[] = [];
  #scopedAfter?: string;
  #scopedLoading = false;

  connectedCallback(): void {
    this.#scope = workspaceScopeFor(location.pathname) ?? { kind: 'platform' };
    this.#scopePath = location.pathname;
    if (!this.client) this.client = this.mode === 'fixture'
      ? new FixtureControlReadClient()
      : new ProductionControlReadClient();
    void this.#start();
  }

  disconnectedCallback(): void {
    this.#generation += 1;
    this.#clearState();
  }

  async #start(): Promise<void> {
    const generation = ++this.#generation;
    this.#clearState();
    const client = this.client;
    this.#renderSessionLoading();
    try {
      const session = await client.session();
      if (!this.#isCurrent(generation, client)) return;
      if (!session.authenticated) {
        this.#renderSignIn();
        return;
      }
      this.#renderDataLoading();
      if (this.#scope.kind === 'platform') await this.#loadPanels(generation, client);
      else await this.#loadScoped(generation, client);
    } catch (error) {
      if (!this.#isCurrent(generation, client)) return;
      if (error instanceof ControlSignInRequiredError) this.#renderSignIn();
      else if (error instanceof ControlForbiddenError) this.#renderForbidden();
      else this.#renderTerminal(errorMessage(error, 'Control session is unavailable.'));
    }
  }

  #clearState(): void {
    this.#editor = undefined;
    this.#overview = undefined;
    this.#overviewRefreshError = undefined;
    this.#tenants = [];
    this.#tenantAfter = undefined;
    this.#tenantError = undefined;
    this.#settings = undefined;
    this.#settingsError = undefined;
    this.#audit = [];
    this.#auditAfter = undefined;
    this.#auditError = undefined;
    this.#tenantLoading = false;
    this.#auditLoading = false;
    this.#fixtureDraft = String(fixturePlatformSetting.initialValue);
    this.#fixtureSimulation = undefined;
    this.#fixtureValidationError = undefined;
    this.#fixtureStatus = undefined;
    this.#scopedItems = [];
    this.#scopedAfter = undefined;
    this.#scopedLoading = false;
  }

  #isCurrent(generation: number, client: ControlReadClient): boolean {
    return this.isConnected && generation === this.#generation && client === this.client &&
      location.pathname === this.#scopePath;
  }

  async #loadScoped(generation: number, client: ControlReadClient): Promise<void> {
    const scope = this.#scope;
    if (scope.kind === 'platform') return;
    try {
      const page = scope.kind === 'tenant'
        ? await client.catalogs(scope.tenant)
        : await client.collections(scope.tenant, scope.catalog);
      if (!this.#isCurrent(generation, client)) return;
      this.#scopedItems = page.items;
      this.#scopedAfter = page.nextAfter;
      this.#renderScoped();
    } catch (error) {
      if (!this.#isCurrent(generation, client)) return;
      this.#scopedItems = [];
      this.#scopedAfter = undefined;
      if (error instanceof ControlSignInRequiredError) this.#renderSignIn();
      else if (error instanceof ControlForbiddenError) this.#renderForbidden();
      else this.#renderTerminal(errorMessage(error, 'Control scope is unavailable.'));
    }
  }

  async #moreScoped(): Promise<void> {
    const scope = this.#scope;
    if (scope.kind === 'platform' || !this.#scopedAfter || this.#scopedLoading) return;
    const after = this.#scopedAfter;
    const generation = this.#generation;
    const client = this.client;
    this.#scopedLoading = true;
    this.#renderScoped();
    try {
      const page = scope.kind === 'tenant'
        ? await client.catalogs(scope.tenant, after)
        : await client.collections(scope.tenant, scope.catalog, after);
      if (!this.#isCurrent(generation, client)) return;
      this.#scopedItems = [...this.#scopedItems, ...page.items] as CatalogView[] | CollectionView[];
      this.#scopedAfter = page.nextAfter;
      this.#scopedLoading = false;
      this.#renderScoped();
      this.querySelector<HTMLElement>('[data-action="more-scoped"], [data-field="scoped-list"]')?.focus();
    } catch (error) {
      if (!this.#isCurrent(generation, client)) return;
      this.#scopedItems = [];
      this.#scopedAfter = undefined;
      this.#scopedLoading = false;
      if (error instanceof ControlSignInRequiredError) this.#renderSignIn();
      else if (error instanceof ControlForbiddenError) this.#renderForbidden();
      else this.#renderTerminal(errorMessage(error, 'Control scope is unavailable.'));
    }
  }

  async #loadPanels(generation: number, client: ControlReadClient): Promise<void> {
    const results = await Promise.allSettled([
      client.overview(),
      client.tenants(),
      client.effectiveSettings(),
      client.audit(),
    ]);
    if (!this.#isCurrent(generation, client)) return;
    const [overview, tenants, settings, audit] = results;
    if (overview.status === 'rejected') {
      if (this.#editor?.isConnected && this.#editor.hasUnresolvedWrite()) {
        const denied = overview.reason instanceof ControlForbiddenError;
        const expired = overview.reason instanceof ControlSignInRequiredError;
        this.#overviewRefreshError = denied
          ? 'Platform access is unavailable. The pending settings request remains in this tab.'
          : expired
            ? 'Sign in is required to refresh platform status. The pending settings request remains in this tab.'
            : 'Platform overview refresh failed. Current values may be stale; the pending settings request remains in this tab.';
        if (denied || expired) {
          this.#overview = undefined;
          this.#tenants = [];
          this.#tenantAfter = undefined;
          this.#tenantError = 'Tenant inventory is unavailable pending authorization.';
          this.#settings = undefined;
          this.#settingsError = 'Effective settings are unavailable pending authorization.';
          this.#audit = [];
          this.#auditAfter = undefined;
          this.#auditError = 'Audit log is unavailable pending authorization.';
        }
        this.#renderWorkspace();
        return;
      }
      if (overview.reason instanceof ControlForbiddenError) {
        this.#renderForbidden();
        return;
      }
      if (overview.reason instanceof ControlSignInRequiredError) {
        this.#renderSignIn();
        return;
      }
      this.#renderTerminal(errorMessage(overview.reason, 'Platform overview is unavailable.'));
      return;
    }
    this.#overview = overview.value;
    this.#overviewRefreshError = undefined;
    this.#acceptTenants(tenants);
    this.#acceptSettings(settings);
    this.#acceptAudit(audit);
    this.#renderWorkspace();
    if (this.mode === 'production' && client instanceof ProductionControlReadClient) {
      this.#mountEditor(client, generation);
    }
  }

  #mountEditor(client: ProductionControlReadClient, generation: number): void {
    const slot = this.querySelector<HTMLElement>('[data-field="platform-editor-slot"]');
    if (!slot || this.#editor) return;
    const editor = document.createElement('tellurion-platform-settings-editor') as TellurionPlatformSettingsEditor;
    editor.client = client;
    this.#editor = editor;
    editor.addEventListener('platform-settings-applied', () => {
      if (this.#isCurrent(generation, client)) void this.#loadPanels(generation, client);
    });
    slot.append(editor);
  }

  #acceptTenants(result: PromiseSettledResult<ControlPage<TenantView>>): void {
    if (result.status === 'fulfilled') {
      this.#tenants = result.value.items;
      this.#tenantAfter = result.value.nextAfter;
      this.#tenantError = undefined;
    } else {
      this.#tenantError = errorMessage(result.reason, 'Tenant list is unavailable.');
    }
  }

  #acceptSettings(result: PromiseSettledResult<EffectiveSettingsView>): void {
    if (result.status === 'fulfilled') {
      this.#settings = result.value;
      this.#settingsError = undefined;
    } else {
      this.#settingsError = errorMessage(result.reason, 'Effective settings are unavailable.');
    }
  }

  #acceptAudit(result: PromiseSettledResult<ControlAuditPage>): void {
    if (result.status === 'fulfilled') {
      this.#audit = result.value.items;
      this.#auditAfter = result.value.nextAfter;
      this.#auditError = undefined;
    } else {
      this.#auditError = errorMessage(result.reason, 'Audit log is unavailable.');
    }
  }

  #renderSessionLoading(): void {
    this.innerHTML = '<section class="control-gate" aria-busy="true"><p role="status">Loading control session…</p></section>';
  }

  #renderDataLoading(): void {
    this.innerHTML = '<section class="control-gate" aria-busy="true"><p role="status">Loading control data…</p></section>';
  }

  #renderSignIn(): void {
    const scoped = this.#scope.kind !== 'platform';
    this.innerHTML = `
      <section class="control-gate">
        <p class="control-gate__eyebrow">Tellurion control</p>
        <h1>Sign in to control Tellurion</h1>
        <p>${scoped ? 'Use an account authorized for this control scope.' : 'Use an authorized platform account to inspect current configuration and propagation state.'}</p>
        <a class="control-button" data-action="sign-in" href="/_auth/control/login?return_to=${escape(scoped ? this.#scopePath : '/ui/control')}">Sign in</a>
      </section>`;
  }

  #renderForbidden(): void {
    this.#renderTerminal(this.#scope.kind === 'platform' ? 'Platform scope unavailable' : 'Control scope unavailable');
  }

  #renderScoped(): void {
    const scope = this.#scope;
    if (scope.kind === 'platform') return;
    const isTenant = scope.kind === 'tenant';
    const title = isTenant ? `${scope.tenant} catalogs` : `${scope.catalog} collections`;
    const coordinate = isTenant ? `/tenants/${scope.tenant}` : `/tenants/${scope.tenant}/catalogs/${scope.catalog}`;
    const entries = this.#scopedItems.map((item) => {
      const href = isTenant
        ? `/ui/control/tenants/${scope.tenant}/catalogs/${item.resource.id}`
        : undefined;
      const name = href
        ? `<a href="${escape(href)}"><code>${escape(item.resource.id)}</code></a>`
        : `<code>${escape(item.resource.id)}</code>`;
      const detail = isTenant ? 'Catalog' : (item as CollectionView).resource.kind;
      return `<li>${name}<span>${escape(detail)}</span></li>`;
    }).join('');
    this.innerHTML = `
      <section class="control-workspace" data-mode="${this.mode}" data-scope="${scope.kind}">
        ${this.mode === 'fixture' ? '<p class="control-demo-boundary">Demonstration data · fixture-only workspace</p>' : ''}
        <nav class="control-nav" aria-label="Workspace navigation">
          <a data-nav="catalog-map" href="${escape(`/ui/${location.search}${location.hash}`)}">Catalog map</a>
          <a aria-current="page" href="${escape(this.#scopePath)}">${escape(title)}</a>
        </nav>
        <main class="control-sheet control-sheet--scoped" aria-live="polite">
          <p class="control-breadcrumb">Control / ${isTenant ? 'tenant' : 'catalog'}</p>
          <h1 id="control-workspace-heading">${escape(title)}</h1>
          <p class="control-coordinate">${escape(coordinate)} · read-only</p>
          <section class="control-sheet__section" aria-labelledby="scoped-items-heading">
            <h2 id="scoped-items-heading">${isTenant ? 'Catalogs' : 'Collections'}</h2>
            ${entries ? `<ul class="control-scope-list control-scope-list--scoped" data-field="scoped-list" tabindex="-1">${entries}</ul>`
              : `<div data-field="scoped-list" tabindex="-1"><p class="control-note">No ${isTenant ? 'catalogs' : 'collections'} are available in this scope.</p></div>`}
            ${this.#scopedLoading ? '<p class="control-note" role="status">Loading more…</p>' : ''}
            ${this.#scopedAfter ? `<button type="button" class="control-more" data-action="more-scoped" ${this.#scopedLoading ? 'disabled' : ''}>Load more</button>` : ''}
          </section>
        </main>
      </section>`;
    this.querySelector<HTMLButtonElement>('[data-action="more-scoped"]')?.addEventListener('click', () => void this.#moreScoped());
  }

  #renderTerminal(message: string): void {
    this.innerHTML = `
      <section class="control-gate control-gate--error">
        <p class="control-gate__eyebrow">Tellurion control</p>
        <h1>${escape(message)}</h1>
        <p>Return to the catalog map or ask a platform administrator to confirm your scope.</p>
        <a class="control-button control-button--quiet" href="/ui/${location.search}${location.hash}">Catalog map</a>
      </section>`;
  }

  #renderWorkspace(): void {
    const overview = this.#overview;
    const revision = overview?.storeRevision ?? '…';
    const applied = overview?.appliedRevision ?? '…';
    const propagation = overview ? (overview.lag === 0 ? 'synchronized' : 'lagging') : 'checking';
    const mapHref = `/ui/${location.search}${location.hash}`;
    const controlHref = `${this.mode === 'fixture' ? '/ui/control-demo' : '/ui/control'}${location.search}${location.hash}`;
    const tenantList = this.#tenantError
      ? `<div data-field="tenant-list" tabindex="-1"><p class="control-note control-note--error">${escape(this.#tenantError)}</p></div>`
      : this.#tenants.length === 0
        ? '<div data-field="tenant-list" tabindex="-1"><p class="control-note">No tenants are available in this inventory.</p></div>'
        : `<ul class="control-scope-list" data-field="tenant-list" tabindex="-1">${this.#tenants.map((tenant) => `
            <li>${this.mode === 'production' && validControlScopeId(tenant.resource.id)
              ? `<a href="${escape(`/ui/control/tenants/${tenant.resource.id}`)}"><code>${escape(tenant.resource.id)}</code></a>`
              : `<code>${escape(tenant.resource.id)}</code>`}<span>revision ${tenant.controlRevision}</span></li>`).join('')}
          </ul>${this.#tenantLoading ? '<p class="control-note" role="status">Loading tenant inventory…</p>' : ''}${this.#tenantAfter ? `<button type="button" class="control-more" data-action="more-tenants" ${this.#tenantLoading ? 'disabled' : ''}>Load another tenant</button>` : ''}`;
    const effective = this.#settingsError
      ? `<p class="control-note control-note--error">${escape(this.#settingsError)}</p>`
      : this.#settings
        ? this.#renderSettings(this.#settings)
        : '<p class="control-note">Loading effective settings…</p>';
    const audit = this.#auditError
      ? `<p class="control-note control-note--error">${escape(this.#auditError)}</p>`
      : this.#renderAudit();
    const markup = `
      <section class="control-workspace" data-mode="${this.mode}">
        ${this.mode === 'fixture' ? '<p class="control-demo-boundary">Demonstration data · fixture-only workspace</p>' : ''}
        <nav class="control-nav" aria-label="Workspace navigation">
          <a data-nav="catalog-map" href="${escape(mapHref)}">Catalog map</a>
          <a data-nav="control" aria-current="page" href="${escape(controlHref)}">Control</a>
        </nav>
        <div class="control-workspace__grid">
          <aside class="control-scope-rail" aria-label="Control scopes">
            <p class="control-label">Scope index</p>
            <button type="button" data-scope="platform" aria-pressed="true">Platform</button>
            <section aria-labelledby="control-tenants-heading">
              <p class="control-label" id="control-tenants-heading">Tenants</p>
              ${tenantList}
            </section>
          </aside>
          <main class="control-sheet" aria-live="polite">
            <p class="control-breadcrumb">Control / platform</p>
            <h1 id="control-workspace-heading" tabindex="-1">Platform control ledger</h1>
            <p class="control-coordinate">/platform · revision ${revision} · applied ${applied} · <span class="control-coordinate__${propagation}">${propagation}</span></p>
            <div data-field="control-refresh-status" role="alert" ${this.#overviewRefreshError ? '' : 'hidden'} class="control-note control-note--error">${this.#overviewRefreshError ? escape(this.#overviewRefreshError) : ''}</div>
            <div data-field="control-ledger">${overview ? this.#renderLedger(overview) : this.#overviewRefreshError
              ? '<p class="control-note">The current platform ledger is unavailable.</p>'
              : '<p class="control-note" role="status">Loading platform overview…</p>'}</div>
            <section class="control-sheet__section" aria-labelledby="effective-settings-heading">
              <div class="control-sheet__section-heading"><p class="control-label">Resolved state</p><h2 id="effective-settings-heading">Effective settings</h2></div>
              ${effective}
            </section>
            ${this.mode === 'production' ? '<div data-field="platform-editor-slot"></div>' : ''}
            ${this.mode === 'fixture' ? this.#renderFixtureSimulator() : ''}
          </main>
          <aside class="control-audit-rail" aria-labelledby="control-audit-heading">
            <p class="control-label">Revision trail</p>
            <h2 id="control-audit-heading">Audit</h2>
            ${audit}
          </aside>
        </div>
      </section>`;
    if (this.#editor?.isConnected) {
      const template = document.createElement('template');
      template.innerHTML = markup;
      for (const selector of [
        '.control-scope-rail', '.control-audit-rail', '.control-coordinate',
        '[data-field="control-refresh-status"]', '[data-field="control-ledger"]',
        '[aria-labelledby="effective-settings-heading"]',
      ]) {
        const current = this.querySelector(selector);
        const next = template.content.querySelector(selector);
        if (current && next) current.replaceWith(next);
      }
      this.querySelector<HTMLButtonElement>('[data-action="more-tenants"]')?.addEventListener('click', () => void this.#moreTenants());
      this.querySelector<HTMLButtonElement>('[data-action="more-audit"]')?.addEventListener('click', () => void this.#moreAudit());
      this.querySelector<HTMLButtonElement>('[data-scope="platform"]')?.addEventListener('click', () => this.#focusHeading());
      return;
    }
    this.innerHTML = markup;
    this.querySelector<HTMLButtonElement>('[data-scope="platform"]')?.addEventListener('click', () => this.#focusHeading());
    this.querySelector<HTMLButtonElement>('[data-action="more-tenants"]')?.addEventListener('click', () => void this.#moreTenants());
    this.querySelector<HTMLButtonElement>('[data-action="more-audit"]')?.addEventListener('click', () => void this.#moreAudit());
    if (this.mode === 'fixture') {
      this.querySelector<HTMLInputElement>('[data-field="fixture-cache-ttl"]')?.addEventListener('input', (event) => {
        const input = event.currentTarget as HTMLInputElement;
        this.#updateFixtureDraft(input.value);
      });
      this.querySelector<HTMLButtonElement>('[data-action="simulate-change"]')?.addEventListener('click', () => this.#simulateFixtureChange());
      this.querySelector<HTMLButtonElement>('[data-action="reset-fixture-draft"]')?.addEventListener('click', () => this.#resetFixtureDraft());
    }
  }

  #renderLedger(overview: ControlOverview): string {
    return `
      <dl class="control-sheet__ledger">
        <div><dt>Store revision</dt><dd>${overview.storeRevision}</dd></div>
        <div><dt>Applied revision</dt><dd>${overview.appliedRevision}</dd></div>
        <div><dt>Propagation lag</dt><dd>${overview.lag}</dd></div>
        <div><dt>Configuration version</dt><dd><code>${escape(overview.configVersion)}</code></dd></div>
        <div><dt>Refresh failures</dt><dd>${overview.pollFailures}</dd></div>
        <div><dt>Activation failures</dt><dd>${overview.activationFailures}</dd></div>
      </dl>`;
  }

  #renderSettings(settings: EffectiveSettingsView): string {
    const rows = Object.entries(settings.effective.settings);
    return `
      <p class="control-note">Effective at applied revision ${settings.appliedRevision}</p>
      ${rows.length === 0 ? '<p class="control-note">No effective setting overrides.</p>' : `
        <dl class="control-settings">${rows.map(([key, setting]) => `<div><dt><code>${escape(key)}</code></dt><dd><code>${escape(settingValue(setting.value))}</code><span class="control-provenance">${escape(provenanceLabel(setting.provenance))}</span></dd></div>`).join('')}</dl>`}`;
  }

  #renderFixtureSimulator(): string {
    const simulation = this.#fixtureSimulation;
    const status = this.#fixtureStatus
      ? `<p data-field="fixture-preview-status" role="status" aria-live="polite" class="control-note">${escape(this.#fixtureStatus)}</p>`
      : '';
    const result = simulation
      ? `<div data-field="fixture-simulation" class="control-note">
          <p><strong>Simulation only:</strong> ${simulation.before} → ${simulation.after} seconds.</p>
          <p>No control revision, audit event, or runtime setting is changed.</p>
          <ul>${simulation.impacts.map((impact) => `<li>${escape(impact)}</li>`).join('')}</ul>
        </div>`
      : '<p class="control-note">This draft is isolated from the running service. Simulate it to inspect a deterministic impact summary.</p>';
    return `
      <section class="control-sheet__section" aria-labelledby="fixture-preview-heading">
        <div class="control-sheet__section-heading"><p class="control-label">Demonstration draft</p><h2 id="fixture-preview-heading">Cache lifetime preview</h2></div>
        <label class="control-note" for="fixture-cache-ttl">Cache lifetime (seconds)</label>
        <input id="fixture-cache-ttl" data-field="fixture-cache-ttl" type="number" min="1" max="86400" step="1" value="${escape(this.#fixtureDraft)}" ${this.#fixtureValidationError ? 'aria-invalid="true" aria-describedby="fixture-cache-ttl-error"' : ''}>
        ${this.#fixtureValidationError ? `<p id="fixture-cache-ttl-error" data-field="fixture-validation" role="alert" class="control-note control-note--error">${escape(this.#fixtureValidationError)}</p>` : ''}
        <p><button type="button" class="control-more" data-action="simulate-change">Simulate change</button> <button type="button" class="control-more" data-action="reset-fixture-draft">Reset draft</button></p>
        ${status}
        ${result}
      </section>`;
  }

  #updateFixtureDraft(value: string): void {
    this.#fixtureDraft = value;
    this.#fixtureSimulation = undefined;
    try {
      simulateFixturePlatformSetting(value);
      this.#fixtureValidationError = undefined;
      this.#fixtureStatus = undefined;
    } catch (error) {
      this.#fixtureValidationError = fixtureValidationMessage(error);
      this.#fixtureStatus = `Draft needs correction: ${this.#fixtureValidationError}`;
    }
    this.#renderWorkspace();
    this.#focusFixtureControl('fixture-cache-ttl');
  }

  #simulateFixtureChange(): void {
    try {
      this.#fixtureSimulation = simulateFixturePlatformSetting(this.#fixtureDraft);
    } catch (error) {
      this.#fixtureSimulation = undefined;
      this.#fixtureValidationError = fixtureValidationMessage(error);
      this.#fixtureStatus = `Draft needs correction: ${this.#fixtureValidationError}`;
      this.#renderWorkspace();
      this.#focusFixtureControl('simulate-change');
      return;
    }
    this.#fixtureValidationError = undefined;
    this.#fixtureStatus = `Simulation complete. Cache lifetime would change from ${this.#fixtureSimulation.before} to ${this.#fixtureSimulation.after} seconds.`;
    this.#renderWorkspace();
    this.#focusFixtureControl('simulate-change');
  }

  #resetFixtureDraft(): void {
    this.#fixtureDraft = String(fixturePlatformSetting.initialValue);
    this.#fixtureSimulation = undefined;
    this.#fixtureValidationError = undefined;
    this.#fixtureStatus = `Draft reset to the ${fixturePlatformSetting.initialValue}-second fixture baseline.`;
    this.#renderWorkspace();
    this.#focusFixtureControl('reset-fixture-draft');
  }

  #focusFixtureControl(field: string): void {
    this.querySelector<HTMLElement>(`[data-field="${field}"], [data-action="${field}"]`)?.focus();
  }

  #renderAudit(): string {
    if (this.#audit.length === 0) return '<p class="control-note">No audit events are available.</p>';
    return `
      <ol class="control-audit-list" data-field="audit-list" tabindex="-1">${this.#audit.map((item) => `
        <li><strong>r${item.revision}</strong><span>${escape(item.method)} ${escape(item.canonicalPath)}</span><code>${escape(item.correlationId)}</code></li>`).join('')}
      </ol>${this.#auditLoading ? '<p class="control-note" role="status">Loading audit events…</p>' : ''}${this.#auditAfter ? `<button type="button" class="control-more" data-action="more-audit" ${this.#auditLoading ? 'disabled' : ''}>Load earlier events</button>` : ''}`;
  }

  #focusHeading(): void {
    this.querySelector<HTMLElement>('#control-workspace-heading')?.focus();
  }

  async #moreTenants(): Promise<void> {
    if (!this.#tenantAfter || this.#tenantLoading) return;
    const generation = this.#generation;
    const client = this.client;
    this.#tenantLoading = true;
    this.#renderWorkspace();
    try {
      const page = await client.tenants(this.#tenantAfter);
      if (!this.#isCurrent(generation, client)) return;
      this.#tenants = [...this.#tenants, ...page.items];
      this.#tenantAfter = page.nextAfter;
    } catch (error) {
      if (!this.#isCurrent(generation, client)) return;
      this.#tenantError = errorMessage(error, 'Tenant list is unavailable.');
    }
    if (!this.#isCurrent(generation, client)) return;
    this.#tenantLoading = false;
    this.#renderWorkspace();
    this.querySelector<HTMLElement>('[data-action="more-tenants"], [data-field="tenant-list"]')?.focus();
  }

  async #moreAudit(): Promise<void> {
    if (!this.#auditAfter || this.#auditLoading) return;
    const generation = this.#generation;
    const client = this.client;
    this.#auditLoading = true;
    this.#renderWorkspace();
    try {
      const page = await client.audit(this.#auditAfter);
      if (!this.#isCurrent(generation, client)) return;
      this.#audit = [...this.#audit, ...page.items];
      this.#auditAfter = page.nextAfter;
    } catch (error) {
      if (!this.#isCurrent(generation, client)) return;
      this.#auditError = errorMessage(error, 'Audit log is unavailable.');
    }
    if (!this.#isCurrent(generation, client)) return;
    this.#auditLoading = false;
    this.#renderWorkspace();
    this.querySelector<HTMLElement>('[data-action="more-audit"], [data-field="audit-list"]')?.focus();
  }
}

if (!customElements.get('tellurion-control-shell')) {
  customElements.define('tellurion-control-shell', TellurionControlShell);
}

export function mountControlWorkspace(root: HTMLElement, mode: ControlMode): void {
  const shell = document.createElement('tellurion-control-shell') as TellurionControlShell;
  shell.mode = mode;
  shell.client = mode === 'fixture' ? new FixtureControlReadClient() : new ProductionControlReadClient();
  root.replaceChildren(shell);
}
