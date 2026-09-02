import '../demo-source-workflow.css';
import {
  configureDemoSource,
  eligibleDemoSourceUrl,
  publishDemoMap,
  resetDemoWorkflow,
  setDemoOpacity,
  setDemoVectorStyle,
  startDemoInspection,
  isVectorDemoSource,
  type DemoWorkflow,
  type ConfiguredDemoWorkflow,
  type InspectedDemoWorkflow,
  type MappedDemoWorkflow,
} from '../lib/demo-source';
import { deleteDemoSource, DemoSourceApiError, registerDemoSource } from '../lib/demo-source-api';
import {
  demoFixtures,
  demoFixtureInventoryError,
  isExecutableFixture,
  isScientificFixture,
  vectorControlsForFixture,
  type DemoFixture,
} from '../lib/demo-fixtures';

const ElementBase: typeof HTMLElement =
  globalThis.HTMLElement ?? (class {} as unknown as typeof HTMLElement);

export function workflowProgress(phase: DemoWorkflow['phase']): Array<{
  label: string;
  current: boolean;
  complete: boolean;
}> {
  const labels = ['Choose', 'Inspect', 'Configure', 'Map'];
  const phaseIndex = phase === 'error' ? 0 : labels.map((label) => label.toLowerCase()).indexOf(phase);
  return labels.map((label, index) => ({
    label,
    current: index === phaseIndex,
    complete: index < phaseIndex,
  }));
}

/** A deliberately short-lived local journey. A successful inspection removes
 * the locator from this element and keeps only the server-issued source id. */
export class TellurionDemoSourceWorkflow extends ElementBase {
  #state: DemoWorkflow = { phase: 'choose', rawUrl: '' };
  #alive = false;
  #connectionGeneration = 0;
  #discardGeneration = 0;
  #inspectionAbort: AbortController | null = null;
  #discarding = false;
  #discardError = '';
  #expiredListener = (event: Event): void => this.#receiveExpiry(event);

  connectedCallback(): void {
    this.#alive = true;
    this.#connectionGeneration += 1;
    document.addEventListener('tellurion-demo-source-expired', this.#expiredListener);
    this.#render();
  }

  disconnectedCallback(): void {
    this.#alive = false;
    this.#connectionGeneration += 1;
    this.#inspectionAbort?.abort();
    document.removeEventListener('tellurion-demo-source-expired', this.#expiredListener);
  }

  #render(): void {
    const stages = workflowProgress(this.#state.phase)
      .map(
        (stage, index) =>
          `<li class="demo-source__stage${stage.current ? ' demo-source__stage--current' : ''}${stage.complete ? ' demo-source__stage--complete' : ''}"${stage.current ? ' aria-current="step"' : ''}><span>${String(index + 1).padStart(2, '0')}</span>${stage.label}</li>`,
      )
      .join('');
    this.innerHTML = `
      <section class="demo-source" aria-labelledby="demo-source-title">
        <header class="demo-source__heading">
          <p class="demo-source__kicker">Direct source field desk</p>
          <h2 id="demo-source-title">Read the map where it lives.</h2>
          <p>Tellurion validates a public HTTPS resource through bounded reads, then maps only its temporary same-origin tile route. No data is copied into this catalog workspace.</p>
        </header>
        <ol class="demo-source__stages" aria-label="Demo source journey">${stages}</ol>
        <div class="demo-source__body" data-field="body"></div>
      </section>
    `;
    const body = this.querySelector<HTMLElement>('[data-field="body"]');
    if (!body) throw new Error('demo workflow body is missing');
    if (this.#state.phase === 'choose' || this.#state.phase === 'error') this.#renderChoose(body);
    if (this.#state.phase === 'inspect') this.#renderInspect(body);
    if (this.#state.phase === 'configure') this.#renderConfigure(body);
    if (this.#state.phase === 'map') this.#renderMap(body);
  }

  #renderChoose(body: HTMLElement): void {
    const error = this.#state.phase === 'error' ? this.#state.message : '';
    const rawUrl = 'rawUrl' in this.#state ? this.#state.rawUrl : '';
    const executableFixtures = demoFixtures.filter(isExecutableFixture);
    const candidateFixtures = demoFixtures.filter((fixture) => !isExecutableFixture(fixture));
    body.innerHTML = `
      <div class="demo-source__choice">
        <section aria-labelledby="demo-source-entry-title">
          <p class="demo-source__section-label">01 / Source</p>
          <h3 id="demo-source-entry-title">Use a known example or your own address</h3>
          <form data-field="source-form" novalidate>
            <label for="demo-source-url">HTTPS resource address</label>
            <div class="demo-source__entry-row">
              <input id="demo-source-url" name="source-url" type="url" inputmode="url" autocomplete="url" maxlength="2048" required aria-describedby="demo-source-help demo-source-status" value="${escapeAttribute(rawUrl)}" />
              <button type="submit">Inspect source</button>
            </div>
            <p id="demo-source-help" class="demo-source__hint">HTTPS only, port 443, no credentials, query string, or fragment. The server performs the final network safety check.</p>
            <p id="demo-source-status" class="demo-source__status${error ? ' demo-source__status--error' : ''}" role="status">${escape(error)}</p>
          </form>
        </section>
        <aside class="demo-source__rules" aria-label="Demo limits">
          <p class="demo-source__section-label">Boundary</p>
          <p>Three temporary sources per browser session. The shared session lasts at most 15 minutes; each source reports its remaining lifetime and is not added to a tenant or catalog.</p>
        </aside>
      </div>
      <section class="demo-source__gallery" aria-labelledby="demo-source-gallery-title">
        <div class="demo-source__gallery-heading"><div><p class="demo-source__section-label">Verified inventory</p><h3 id="demo-source-gallery-title">Formats on the route</h3></div><p>${demoFixtureInventoryError ? escape(demoFixtureInventoryError) : `${executableFixtures.length} publisher-verified examples are ready to inspect.`}</p></div>
        <ul>${executableFixtures.map((fixture) => this.#fixtureRow(fixture)).join('')}</ul>
        ${candidateFixtures.length ? `<details class="demo-source__candidates"><summary>${candidateFixtures.length} planned or review-gated formats</summary><ul>${candidateFixtures.map((fixture) => this.#fixtureRow(fixture)).join('')}</ul></details>` : ''}
      </section>
    `;
    const form = body.querySelector<HTMLFormElement>('[data-field="source-form"]');
    const input = body.querySelector<HTMLInputElement>('#demo-source-url');
    if (!form || !input) throw new Error('demo source form is missing');
    form.addEventListener('submit', (event) => {
      event.preventDefault();
      void this.#inspect(input.value, input, form);
    });
    body.querySelectorAll<HTMLButtonElement>('[data-example-id]').forEach((button) => {
      button.addEventListener('click', () => {
        const fixture = demoFixtures.find((candidate) => candidate.id === button.dataset.exampleId);
        if (!fixture) return;
        if (isExecutableFixture(fixture)) {
          input.value = fixture.url;
          input.focus();
          void this.#inspect(fixture.url, input, form);
        } else {
          body.querySelector<HTMLElement>('#demo-source-status')!.textContent = `${fixture.connector.reason} This example is listed for format coverage only.`;
        }
      });
    });
  }

  #fixtureRow(fixture: DemoFixture): string {
    const license =
      fixture.license.verification === 'confirmed' && fixture.license.termsUrl
        ? `<a href="${escapeAttribute(fixture.license.termsUrl)}" target="_blank" rel="noreferrer">${escape(fixture.license.label)}</a>`
        : escape(fixture.license.label);
    const executable = isExecutableFixture(fixture);
    const action = executable
      ? `<button type="button" data-example-id="${escapeAttribute(fixture.id)}">Use this example</button>`
      : `<button type="button" data-example-id="${escapeAttribute(fixture.id)}" aria-describedby="fixture-${escapeAttribute(fixture.id)}" disabled>Connector queued</button>`;
    const scientific = isScientificFixture(fixture)
      ? '<p class="demo-source__fixture-note">A 2D variable and its spatial axes slice must be selected before this scientific source can be mapped.</p>'
      : '';
    const vector = vectorControlsForFixture(fixture)
      ? `<p class="demo-source__fixture-note">${executable ? 'Detected field metadata and local style controls are available after inspection.' : 'Vector presentation controls will appear when this connector is available.'}</p>`
      : '';
    const identity = fixture.content.expectedStrongEtag && fixture.content.expectedLength
      ? `<p class="demo-source__fixture-facts">${fixture.content.expectedLength.toLocaleString()} bytes · ${escape(fixture.content.expectedStrongEtag)}</p>`
      : '';
    const resource = fixture.resource
      ? `<p class="demo-source__fixture-facts">${escape(fixture.resource.crs)} · ${escape(fixture.resource.testedInitialView ?? fixture.resource.selected)}</p>`
      : '';
    return `<li class="demo-source__fixture${executable ? ' demo-source__fixture--ready' : ''}">
      <div><p class="demo-source__fixture-meta">${escape(fixture.format)} · ${escape(fixture.transport)}</p><h4>${escape(fixture.title)}</h4><p>${escape(fixture.provider)} · ${license} · <a href="${escapeAttribute(fixture.sourcePage)}" target="_blank" rel="noreferrer">Source details</a></p>${identity}${resource}<p id="fixture-${escapeAttribute(fixture.id)}">${escape(fixture.connector.reason)}</p>${scientific}${vector}</div>${action}
    </li>`;
  }

  async #inspect(rawUrl: string, input: HTMLInputElement, form: HTMLFormElement): Promise<void> {
    const eligible = eligibleDemoSourceUrl(rawUrl);
    const status = this.querySelector<HTMLElement>('#demo-source-status');
    if (!eligible.ok) {
      if (status) {
        status.textContent = eligible.message;
        status.classList.add('demo-source__status--error');
      }
      input.focus();
      return;
    }
    const submit = form.querySelector<HTMLButtonElement>('button[type="submit"]');
    if (submit) submit.disabled = true;
    if (status) {
      status.classList.remove('demo-source__status--error');
      status.textContent = 'Inspecting through the bounded source gateway…';
    }
    const attempt = startDemoInspection(eligible.value);
    const controller = new AbortController();
    const generation = this.#connectionGeneration;
    this.#inspectionAbort = controller;
    try {
      const source = await registerDemoSource(eligible.value, controller.signal);
      if (!this.#alive || generation !== this.#connectionGeneration) {
        void deleteDemoSource(source).catch(() => undefined);
        return;
      }
      // Clear both the DOM control and the only workflow field that held it
      // before rendering an opaque, server-returned inspection.
      input.value = '';
      const curated = demoFixtures.find((fixture) =>
        isExecutableFixture(fixture) && fixture.url === eligible.value,
      );
      this.#state = attempt.succeed(curated ? { ...source, attribution: curated.attribution } : source);
      this.#render();
      this.querySelector<HTMLElement>('[data-field="inspection-title"]')?.focus();
    } catch (error) {
      if (!this.#alive || generation !== this.#connectionGeneration) return;
      const statusCode = error instanceof DemoSourceApiError ? error.status : 0;
      this.#state = attempt.fail(statusCode || 503);
      this.#render();
      this.querySelector<HTMLInputElement>('#demo-source-url')?.focus();
    } finally {
      if (this.#inspectionAbort === controller) this.#inspectionAbort = null;
    }
  }

  #renderInspect(body: HTMLElement): void {
    const state = this.#state as InspectedDemoWorkflow;
    const source = state.source;
    body.innerHTML = `
      <section class="demo-source__inspection" aria-labelledby="demo-source-inspection-title" tabindex="-1" data-field="inspection-title">
        <p class="demo-source__section-label">02 / Inspection</p>
        <h3 id="demo-source-inspection-title">A temporary source is ready.</h3>
        <dl>
          <div><dt>Format</dt><dd>${escape(source.format)}</dd></div><div><dt>Read mode</dt><dd>${escape(source.transport)}</dd></div><div><dt>Revision</dt><dd>${escape(source.revision)}</dd></div><div><dt>Lifetime</dt><dd>${remainingLifetime(source.limits.expires_in_seconds)}</dd></div>
          <div><dt>Geometry / CRS</dt><dd>${source.geometryType ? `${escape(source.geometryType)} / EPSG:${source.srid}` : 'Server-validated tiled raster'}</dd></div><div><dt>Extent</dt><dd>${extentText(source.extent)}</dd></div>
          ${source.numberMatched === null ? '' : `<div><dt>Features</dt><dd>${source.numberMatched.toLocaleString()}</dd></div>`}
          ${source.properties.length ? `<div><dt>Fields</dt><dd>${escape(source.properties.join(', '))}</dd></div>` : ''}
        </dl>
        <p class="demo-source__attribution">${escape(source.attribution)}</p>
        ${this.#discardError ? `<p class="demo-source__status demo-source__status--error" role="status">${escape(this.#discardError)}</p>` : ''}
        <div class="demo-source__actions"><button type="button" data-action="configure"${this.#discarding ? ' disabled' : ''}>Configure map</button><button type="button" class="demo-source__quiet" data-action="reset"${this.#discarding ? ' disabled' : ''}>${this.#discarding ? 'Discarding…' : 'Discard source'}</button></div>
      </section>`;
    this.#attachActions(body);
  }

  #renderConfigure(body: HTMLElement): void {
    const state = this.#state as ConfiguredDemoWorkflow;
    const vector = isVectorDemoSource(state.source);
    const vectorControls = vector ? `
        <p>The inspected fields are metadata in this preview. The map applies a local display style to the server-advertised vector tiles; it does not change the source or tile payload.</p>
        <label for="demo-source-style">Map style</label>
        <select id="demo-source-style"><option value="survey-ink"${state.style === 'survey-ink' ? ' selected' : ''}>Survey ink</option><option value="coastline-signal"${state.style === 'coastline-signal' ? ' selected' : ''}>Coastline signal</option></select>
      ` : `
        <p>This is the only server-validated render profile for this slice. Opacity changes the local map presentation; it does not edit the source or create a server-side style.</p>
        <label for="demo-source-opacity">Opacity <output data-field="opacity-value">${Math.round(state.opacity * 100)}%</output></label>
        <input id="demo-source-opacity" type="range" min="0" max="1" step="0.05" value="${state.opacity}" />
      `;
    body.innerHTML = `
      <section class="demo-source__configure" aria-labelledby="demo-source-configure-title">
        <p class="demo-source__section-label">03 / Map setup</p><h3 id="demo-source-configure-title">${vector ? 'Vector presentation' : 'Categorical land-cover palette'}</h3>
        ${vectorControls}
        <p class="demo-source__cache"><strong>Runtime facts</strong> Source metadata and any bounded spool are scoped to the temporary session and revision. Tile responses are marked private, no-store; this public preview has no configurable tile cache.</p>
        <p class="demo-source__boundary">Write, indexing, reprojection, and persistent configuration controls are not available in this temporary preview.</p>
        ${this.#discardError ? `<p class="demo-source__status demo-source__status--error" role="status">${escape(this.#discardError)}</p>` : ''}
        <div class="demo-source__actions"><button type="button" data-action="map"${this.#discarding ? ' disabled' : ''}>Open temporary map</button><button type="button" class="demo-source__quiet" data-action="reset"${this.#discarding ? ' disabled' : ''}>${this.#discarding ? 'Discarding…' : 'Discard source'}</button></div>
      </section>`;
    const range = body.querySelector<HTMLInputElement>('#demo-source-opacity');
    range?.addEventListener('input', () => {
      this.#state = setDemoOpacity(this.#state, Number(range.value));
      const output = body.querySelector<HTMLOutputElement>('[data-field="opacity-value"]');
      if (output && (this.#state.phase === 'configure' || this.#state.phase === 'inspect' || this.#state.phase === 'map')) output.value = `${Math.round(this.#state.opacity * 100)}%`;
    });
    body.querySelector<HTMLSelectElement>('#demo-source-style')?.addEventListener('change', (event) => {
      const selected = (event.currentTarget as HTMLSelectElement).value;
      this.#state = setDemoVectorStyle(this.#state, selected === 'coastline-signal' ? selected : 'survey-ink');
    });
    this.#attachActions(body);
  }

  #renderMap(body: HTMLElement): void {
    const state = this.#state as MappedDemoWorkflow;
    const source = state.source;
    body.innerHTML = `<section class="demo-source__map-ready" aria-labelledby="demo-source-map-title"><p class="demo-source__section-label">04 / Map</p><h3 id="demo-source-map-title">Temporary layer opened in the catalog map.</h3><p>${escape(source.attribution)}</p><p>The layer uses the server-advertised, same-origin tile link and will disappear with this source session.</p>${this.#discardError ? `<p class="demo-source__status demo-source__status--error" role="status">${escape(this.#discardError)}</p>` : ''}<button type="button" class="demo-source__quiet" data-action="reset"${this.#discarding ? ' disabled' : ''}>${this.#discarding ? 'Removing…' : 'Remove temporary layer'}</button></section>`;
    this.#attachActions(body);
  }

  #attachActions(body: HTMLElement): void {
    body.querySelector<HTMLButtonElement>('[data-action="configure"]')?.addEventListener('click', () => {
      this.#state = configureDemoSource(this.#state);
      this.#render();
    });
    body.querySelector<HTMLButtonElement>('[data-action="map"]')?.addEventListener('click', () => {
      const mapState = publishDemoMap(this.#state) as MappedDemoWorkflow;
      this.#state = mapState;
      this.dispatchEvent(new CustomEvent('tellurion-demo-map', { bubbles: true, composed: true, detail: { source: mapState.source, opacity: mapState.opacity, style: mapState.style } }));
      this.#render();
    });
    body.querySelector<HTMLButtonElement>('[data-action="reset"]')?.addEventListener('click', () => {
      void this.#discard();
    });
  }

  async #discard(): Promise<void> {
    if (this.#discarding) return;
    const source = this.#state.phase === 'inspect' || this.#state.phase === 'configure' || this.#state.phase === 'map' ? this.#state.source : null;
    if (!source) return;
    const requestGeneration = ++this.#discardGeneration;
    this.#discarding = true;
    this.#discardError = '';
    this.#render();
    try {
      await deleteDemoSource(source);
      if (requestGeneration !== this.#discardGeneration || !this.#stateHasSource(source.id)) return;
      document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', { detail: { sourceId: source.id } }));
      this.#state = resetDemoWorkflow(this.#state);
      this.#discarding = false;
      this.#discardError = '';
      if (this.#alive) {
        this.#render();
        this.querySelector<HTMLInputElement>('#demo-source-url')?.focus();
      }
    } catch {
      if (requestGeneration !== this.#discardGeneration || !this.#stateHasSource(source.id)) return;
      this.#discarding = false;
      this.#discardError = 'The temporary source could not be discarded. Retry removal.';
      if (this.#alive) this.#render();
    }
  }

  #stateHasSource(sourceId: string): boolean {
    return (this.#state.phase === 'inspect' || this.#state.phase === 'configure' || this.#state.phase === 'map') &&
      this.#state.source.id === sourceId;
  }

  #receiveExpiry(event: Event): void {
    if (!(event instanceof CustomEvent)) return;
    const sourceId = (event.detail as { sourceId?: unknown } | null)?.sourceId;
    if (typeof sourceId !== 'string' || !this.#stateHasSource(sourceId)) return;
    this.#discardGeneration += 1;
    this.#discarding = false;
    this.#discardError = '';
    this.#state = resetDemoWorkflow(this.#state);
    if (this.#alive) {
      this.#render();
      this.querySelector<HTMLInputElement>('#demo-source-url')?.focus();
    }
  }
}

function extentText(extent: [number, number, number, number] | null): string {
  return extent ? `${extent.join(', ')} (reported CRS)` : 'Not reported';
}

function remainingLifetime(seconds: number): string {
  const minutes = Math.max(1, Math.ceil(seconds / 60));
  return `${minutes} minute${minutes === 1 ? '' : 's'} remaining`;
}

function escape(value: string): string {
  return value.replace(/[&<>"']/g, (character) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[character]!);
}

function escapeAttribute(value: string): string {
  return escape(value);
}

if (globalThis.customElements && !globalThis.customElements.get('tellurion-demo-source-workflow')) {
  globalThis.customElements.define('tellurion-demo-source-workflow', TellurionDemoSourceWorkflow);
}
