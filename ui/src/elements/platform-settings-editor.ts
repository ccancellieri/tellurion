import {
  ControlApiError,
  ProductionControlReadClient,
  controlSettingsScopeDescriptor,
  type ControlSettingsEditScope,
} from '../lib/control-api';
import { PlatformSettingsEditSession, type PlatformSettingsEditView } from '../lib/platform-settings-session';

function escape(value: unknown): string {
  return String(value).replaceAll('&', '&amp;').replaceAll('<', '&lt;')
    .replaceAll('>', '&gt;').replaceAll('"', '&quot;').replaceAll("'", '&#39;');
}

function cacheValue(view: PlatformSettingsEditView): string {
  const value = view.draft?.cache_ttl_s;
  return value === null || value === undefined ? '' : String(value);
}

function parseCacheValue(raw: string): number | null {
  if (raw.trim() === '') return null;
  if (!/^[0-9]+$/.test(raw)) throw new ControlApiError(400, 'Enter a non-negative whole cache lifetime.');
  const value = Number(raw);
  if (!Number.isSafeInteger(value)) throw new ControlApiError(400, 'Enter a non-negative whole cache lifetime.');
  return value;
}

/** Production-only view over the durable settings edit session. */
export class TellurionPlatformSettingsEditor extends HTMLElement {
  client!: ProductionControlReadClient;
  scope: ControlSettingsEditScope = { kind: 'platform' };
  #session?: PlatformSettingsEditSession;
  #resourceKey = 'platform';
  #generation = 0;
  #validation?: string;
  #error?: string;
  #lastCommit?: { revision: number; changedResources: string[] };
  readonly #beforeUnload = (event: BeforeUnloadEvent): void => {
    if (!this.hasUnresolvedWrite()) return;
    event.preventDefault();
    event.returnValue = '';
  };

  connectedCallback(): void {
    if (!(this.client instanceof ProductionControlReadClient)) return;
    const generation = ++this.#generation;
    this.#resourceKey = controlSettingsScopeDescriptor(this.scope).resourceKey;
    this.#session = new PlatformSettingsEditSession(this.client, undefined, this.scope);
    this.#validation = undefined;
    this.#error = undefined;
    this.#lastCommit = undefined;
    window.addEventListener('beforeunload', this.#beforeUnload);
    this.#render();
    void this.#load(generation);
  }

  disconnectedCallback(): void {
    ++this.#generation;
    window.removeEventListener('beforeunload', this.#beforeUnload);
    this.#session = undefined;
  }

  hasUnresolvedWrite(): boolean {
    const phase = this.#session?.view().phase;
    return phase === 'applying' || phase === 'uncertain';
  }

  #isCurrent(generation: number, session: PlatformSettingsEditSession): boolean {
    return this.isConnected && generation === this.#generation && session === this.#session;
  }

  async #load(generation: number): Promise<boolean> {
    const session = this.#session!;
    const work = session.load();
    this.#render();
    try {
      await work;
      if (!this.#isCurrent(generation, session)) return false;
      this.#error = undefined;
      this.#render();
      return true;
    } catch (error) {
      if (!this.#isCurrent(generation, session)) return false;
      this.#error = this.#safeError(error, 'Control settings are unavailable.');
      this.#render();
      return false;
    }
  }

  #safeError(error: unknown, fallback: string): string {
    return error instanceof ControlApiError ? error.message : fallback;
  }

  #render(): void {
    const view = this.#session?.view();
    if (!view) return;
    const loading = view.phase === 'unloaded' || view.phase === 'loading';
    const editable = view.phase === 'editing' || view.phase === 'previewed';
    const label = this.scope.kind === 'platform' ? 'Platform' : this.scope.kind === 'tenant' ? 'Tenant' : 'Catalog';
    const description = this.scope.kind === 'platform'
      ? 'Platform-wide changes become inherited defaults for tenant, catalog, and collection settings unless overridden. A changed control revision does not guarantee runtime activation.'
      : `Changes to this ${this.scope.kind} cache lifetime do not alter sibling scopes. A changed control revision does not guarantee runtime activation.`;
    this.innerHTML = `
      <section class="control-sheet__section control-editor" aria-labelledby="control-settings-heading" ${loading ? 'aria-busy="true"' : ''}>
        <div class="control-sheet__section-heading"><p class="control-label">Durable settings</p><h2 id="control-settings-heading">${label} settings</h2></div>
        <p class="control-note">${description}</p>
        ${view.entityVersion ? `<p class="control-note">Entity version <code>${escape(view.entityVersion)}</code> · source revision ${escape(view.controlRevision)}</p>` : ''}
        ${loading ? `<p class="control-note" role="status">Loading ${label.toLowerCase()} settings…</p>` : view.draft ? `
          <label for="control-cache-ttl">Cache lifetime (seconds)</label>
          <input id="control-cache-ttl" data-field="cache-ttl" type="text" inputmode="numeric" value="${escape(cacheValue(view))}" ${editable ? '' : 'disabled'} aria-describedby="control-cache-help${this.#validation ? ' control-cache-error' : ''}" ${this.#validation ? 'aria-invalid="true"' : ''}>
          <p id="control-cache-help" class="control-note">Use a non-negative whole number, including 0. Leave blank to unset this scope's value.</p>
        ` : ''}
        <div data-field="editor-feedback" aria-live="polite">${this.#feedback()}</div>
        <div data-field="editor-actions" class="control-editor__actions">${this.#actions(view)}</div>
      </section>`;
    this.querySelector<HTMLInputElement>('[data-field="cache-ttl"]')?.addEventListener('input', (event) => {
      this.#edit((event.currentTarget as HTMLInputElement).value);
    });
    this.#bindActions();
  }

  #feedback(): string {
    const view = this.#session?.view();
    const preview = view?.preview;
    return `
      ${this.#validation ? `<p id="control-cache-error" role="alert" class="control-note control-note--error">${escape(this.#validation)}</p>` : ''}
      ${this.#error ? `<p role="alert" class="control-note control-note--error">${escape(this.#error)}</p>` : ''}
      ${view?.needsRebase ? '<p class="control-note">The settings entity changed. Your draft is retained; rebase it before previewing again.</p>' : ''}
      ${view?.phase === 'uncertain' ? '<p class="control-note control-note--error" role="alert">Apply outcome is uncertain. Retry the exact request before editing or leaving.</p>' : ''}
      ${view?.phase === 'applying' ? '<p class="control-note" role="status">Applying the previewed settings…</p>' : ''}
      ${preview ? `<p class="control-note" data-field="preview-summary">Revision ${escape(preview.baseRevision)} → ${escape(preview.prospectiveRevision)} · changed resource ${escape(preview.changedResources.join(', '))} · proposed entity version ${escape(preview.entityVersions[this.#resourceKey])}</p>` : ''}
      ${this.#lastCommit ? `<p class="control-note" role="status">Changed ${escape(this.#lastCommit.changedResources.join(', '))} at revision ${escape(this.#lastCommit.revision)}. ${view?.phase === 'loading' ? 'Reloading settings…' : this.#error ? 'Reload failed; the changed revision is recorded above.' : 'Settings reloaded.'}</p>` : ''}`;
  }

  #actions(view: PlatformSettingsEditView): string {
    if (view.phase === 'uncertain') return '<button type="button" class="control-more" data-action="retry">Retry exact apply</button>';
    if (view.phase === 'applied' || view.phase === 'loading' || view.phase === 'previewing' || view.phase === 'applying') return '';
    if (view.needsRebase) return '<button type="button" class="control-more" data-action="rebase">Rebase retained draft</button>';
    if (view.phase === 'previewed') return '<button type="button" class="control-more" data-action="apply">Apply previewed settings</button>';
    if (view.phase === 'editing') return '<button type="button" class="control-more" data-action="preview">Preview changes</button>';
    return '';
  }

  #bindActions(): void {
    this.querySelector<HTMLButtonElement>('[data-action="preview"]')?.addEventListener('click', () => void this.#preview());
    this.querySelector<HTMLButtonElement>('[data-action="apply"]')?.addEventListener('click', () => void this.#apply(false));
    this.querySelector<HTMLButtonElement>('[data-action="retry"]')?.addEventListener('click', () => void this.#apply(true));
    this.querySelector<HTMLButtonElement>('[data-action="rebase"]')?.addEventListener('click', () => void this.#rebase());
  }

  #refreshFeedback(): void {
    const view = this.#session?.view();
    const feedback = this.querySelector<HTMLElement>('[data-field="editor-feedback"]');
    const actions = this.querySelector<HTMLElement>('[data-field="editor-actions"]');
    if (!view || !feedback || !actions) return;
    feedback.innerHTML = this.#feedback();
    actions.innerHTML = this.#actions(view);
    this.#bindActions();
    const input = this.querySelector<HTMLInputElement>('[data-field="cache-ttl"]');
    if (input) {
      input.setAttribute('aria-describedby', `control-cache-help${this.#validation ? ' control-cache-error' : ''}`);
      if (this.#validation) input.setAttribute('aria-invalid', 'true');
      else input.removeAttribute('aria-invalid');
    }
  }

  #edit(raw: string): void {
    const session = this.#session;
    if (!session) return;
    const phase = session.view().phase;
    if (phase !== 'editing' && phase !== 'previewed') return;
    this.#error = undefined;
    this.#lastCommit = undefined;
    try {
      session.editCacheTtlSeconds(parseCacheValue(raw));
      this.#validation = undefined;
    } catch (error) {
      session.editCacheTtlSeconds(null);
      this.#validation = this.#safeError(error, 'Enter a non-negative whole cache lifetime.');
    }
    this.#refreshFeedback();
  }

  async #preview(): Promise<void> {
    const session = this.#session;
    if (!session || session.view().phase !== 'editing' || this.#validation) return;
    const generation = this.#generation;
    this.#error = undefined;
    const work = session.preview();
    this.#render();
    try {
      await work;
      if (!this.#isCurrent(generation, session)) return;
      this.#render();
      this.querySelector<HTMLElement>('[data-action="apply"]')?.focus();
    } catch (error) {
      if (!this.#isCurrent(generation, session)) return;
      this.#error = this.#safeError(error, 'Control settings preview is unavailable.');
      this.#render();
      this.querySelector<HTMLElement>('[data-action="rebase"], [data-action="preview"]')?.focus();
    }
  }

  async #rebase(): Promise<void> {
    const session = this.#session;
    if (!session || !session.view().needsRebase) return;
    const generation = this.#generation;
    this.#error = undefined;
    const work = session.rebaseAfterConflict();
    this.#render();
    try {
      await work;
      if (!this.#isCurrent(generation, session)) return;
      this.#render();
      this.querySelector<HTMLElement>('[data-field="cache-ttl"]')?.focus();
    } catch (error) {
      if (!this.#isCurrent(generation, session)) return;
      this.#error = this.#safeError(error, 'Control settings could not be rebased.');
      this.#render();
    }
  }

  async #apply(retry: boolean): Promise<void> {
    const session = this.#session;
    if (!session || (retry ? session.view().phase !== 'uncertain' : session.view().phase !== 'previewed')) return;
    const generation = this.#generation;
    this.#error = undefined;
    const work = retry ? session.retryUncertainApply() : session.apply();
    this.#render();
    try {
      const commit = await work;
      if (!this.#isCurrent(generation, session)) return;
      this.#lastCommit = commit;
      this.#render();
      const reloaded = await this.#load(generation);
      if (reloaded && this.#isCurrent(generation, session)) this.dispatchEvent(new CustomEvent('control-settings-applied', { bubbles: true }));
    } catch (error) {
      if (!this.#isCurrent(generation, session)) return;
      this.#error = this.#safeError(error, 'Control settings apply is unavailable.');
      this.#render();
      this.querySelector<HTMLElement>('[data-action="retry"], [data-action="preview"]')?.focus();
    }
  }
}

if (!customElements.get('tellurion-platform-settings-editor')) {
  customElements.define('tellurion-platform-settings-editor', TellurionPlatformSettingsEditor);
}
