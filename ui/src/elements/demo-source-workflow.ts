import '../demo-source-workflow.css';
import {
  configureDemoSource,
  eligibleDemoSourceUrl,
  publishDemoMap,
  resetDemoWorkflow,
  setDemoOpacity,
  setDemoRasterProfile,
  setDemoVectorStyle,
  sourceForDemoRasterProfile,
  startDemoInspection,
  isVectorDemoSource,
  type DemoWorkflow,
  type ConfiguredDemoWorkflow,
  type DemoSourceProvenance,
  type InspectedDemoWorkflow,
  type MappedDemoWorkflow,
} from '../lib/demo-source';
import { deleteDemoSource, DemoSourceApiError, registerDemoSource } from '../lib/demo-source-api';
import { validateStacAssetPreview } from '../lib/stac-api';
import type { StacAssetPreviewResult } from '../lib/stac-preview-events';
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
  #inspectionGeneration = 0;
  #inspectionAbort: AbortController | null = null;
  #discarding = false;
  #discardError = '';
  #workflowStatus = '';
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
    this.#inspectionGeneration += 1;
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
        <p class="demo-source__status${this.#workflowStatus ? '' : ' demo-source__status--empty'}" data-field="workflow-status" role="status" tabindex="-1">${escape(this.#workflowStatus)}</p>
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
    this.#workflowStatus = '';
    const result = await this.#registerInspection(eligible.value, eligible.value, undefined, null);
    if (result === 'accepted') {
      // Clear the detached control too: no browser-owned copy survives the
      // successful handoff into opaque source state.
      input.value = '';
    }
  }

  async previewStacAsset(value: unknown): Promise<StacAssetPreviewResult> {
    const descriptor = validateStacAssetPreview(value);
    if (!descriptor) {
      this.#workflowStatus = 'This STAC asset preview is no longer valid. Search again.';
      this.#render();
      this.#focusWorkflowStatus();
      return 'invalid';
    }
    if (this.#state.phase === 'inspect' || this.#state.phase === 'configure' || this.#state.phase === 'map') {
      this.#workflowStatus = 'Discard the current temporary source before previewing another STAC asset.';
      this.#render();
      this.#focusWorkflowStatus();
      return 'busy';
    }
    const provenance: DemoSourceProvenance = {
      itemId: descriptor.itemId,
      collectionId: descriptor.collectionId,
      assetKey: descriptor.assetKey,
      assetLabel: descriptor.assetLabel,
      mediaType: descriptor.mediaType,
      roles: [...descriptor.roles],
      itemHref: descriptor.itemHref,
      assetDescription: descriptor.assetDescription,
    };
    this.#workflowStatus = `Inspecting STAC asset “${descriptor.assetLabel}” through the bounded source gateway…`;
    this.#render();
    this.#focusWorkflowStatus();
    return this.#registerInspection(descriptor.url, '', descriptor.format, provenance);
  }

  async #registerInspection(
    url: string,
    rawUrlOnFailure: string,
    format: import('../lib/demo-source-api').DemoSourceRegistrationFormat | undefined,
    provenance: DemoSourceProvenance | null,
  ): Promise<StacAssetPreviewResult> {
    this.#inspectionAbort?.abort();
    const controller = new AbortController();
    const connectionGeneration = this.#connectionGeneration;
    const inspectionGeneration = ++this.#inspectionGeneration;
    this.#inspectionAbort = controller;
    const attempt = startDemoInspection(rawUrlOnFailure, provenance);
    try {
      const source = format
        ? await registerDemoSource(url, controller.signal, format)
        : await registerDemoSource(url, controller.signal);
      if (!this.#alive || connectionGeneration !== this.#connectionGeneration ||
          inspectionGeneration !== this.#inspectionGeneration) {
        await deleteDemoSource(source).catch(() => undefined);
        return 'failed';
      }
      const curated = demoFixtures.find((fixture) =>
        isExecutableFixture(fixture) && fixture.url === url,
      );
      this.#state = attempt.succeed(curated ? { ...source, attribution: curated.attribution } : source);
      this.#workflowStatus = provenance ? 'STAC asset inspection complete.' : '';
      this.#render();
      this.querySelector<HTMLElement>('[data-field="inspection-title"]')?.focus();
      return 'accepted';
    } catch (error) {
      if (!this.#alive || connectionGeneration !== this.#connectionGeneration ||
          inspectionGeneration !== this.#inspectionGeneration) return 'failed';
      const statusCode = error instanceof DemoSourceApiError ? error.status : 0;
      this.#state = attempt.fail(statusCode || 503);
      this.#workflowStatus = provenance ? 'The STAC asset was not accepted for temporary inspection.' : '';
      this.#render();
      this.querySelector<HTMLInputElement>('#demo-source-url')?.focus();
      return 'failed';
    } finally {
      if (this.#inspectionAbort === controller) this.#inspectionAbort = null;
    }
  }

  #focusWorkflowStatus(): void {
    this.querySelector<HTMLElement>('[data-field="workflow-status"]')?.focus();
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
          <div><dt>Geometry / CRS</dt><dd>${source.geometryType ? `${escape(source.geometryType)} / EPSG:${source.srid}` : source.raster ? `Tiled raster / EPSG:${source.raster.epsg}` : 'Server-validated tiled raster'}</dd></div><div><dt>Extent</dt><dd>${extentText(source.extent, source.raster?.epsg ?? source.srid)}</dd></div>
          ${source.raster ? `<div><dt>Raster layout</dt><dd>${escape(source.raster.layout.toUpperCase())} · ${source.raster.channelCount} channel${source.raster.channelCount === 1 ? '' : 's'} · ${source.raster.bitsPerSample}-bit</dd></div><div><dt>Native shape</dt><dd>${source.raster.shape[0].toLocaleString()} × ${source.raster.shape[1].toLocaleString()} px</dd></div><div><dt>Pixel size</dt><dd>${source.raster.pixelSize.map(formatRasterNumber).join(' × ')}°</dd></div><div><dt>Levels</dt><dd>${source.raster.levels.length} (${Math.max(0, source.raster.levels.length - 1)} overviews)</dd></div><div><dt>Nodata</dt><dd>${rasterNodataText(source.raster.nodata)}</dd></div>${source.raster.unit ? `<div><dt>Unit</dt><dd>${escape(source.raster.unit)}</dd></div>` : ''}` : ''}
          ${source.numberMatched === null ? '' : `<div><dt>Features</dt><dd>${source.numberMatched.toLocaleString()}</dd></div>`}
          ${source.properties.length ? `<div><dt>Fields</dt><dd>${escape(source.properties.join(', '))}</dd></div>` : ''}
        </dl>
        ${state.provenance ? provenanceMarkup(state.provenance) : ''}
        ${source.raster?.legend.length ? `<details class="demo-source__legend"><summary>Embedded palette · ${source.raster.legend.length} source entries</summary><ul>${source.raster.legend.map((entry) => `<li><span style="--legend-color: ${legendColor(entry.rgba)}"></span><code>${entry.value}</code></li>`).join('')}</ul></details>` : ''}
        <p class="demo-source__attribution">${escape(source.attribution)}</p>
        ${this.#discardError ? `<p class="demo-source__status demo-source__status--error" role="status">${escape(this.#discardError)}</p>` : ''}
        <div class="demo-source__actions"><button type="button" data-action="configure"${this.#discarding ? ' disabled' : ''}>Configure map</button><button type="button" class="demo-source__quiet" data-action="reset"${this.#discarding ? ' disabled' : ''}>${this.#discarding ? 'Discarding…' : 'Discard source'}</button></div>
      </section>`;
    this.#attachActions(body);
  }

  #renderConfigure(body: HTMLElement): void {
    const state = this.#state as ConfiguredDemoWorkflow;
    const vector = isVectorDemoSource(state.source);
    const raster = state.source.raster;
    const vectorControls = vector ? `
        <p>The inspected fields are metadata in this preview. The map applies a local display style to the server-advertised vector tiles; it does not change the source or tile payload.</p>
        <label for="demo-source-style">Map style</label>
        <select id="demo-source-style"><option value="survey-ink"${state.style === 'survey-ink' ? ' selected' : ''}>Survey ink</option><option value="coastline-signal"${state.style === 'coastline-signal' ? ' selected' : ''}>Coastline signal</option></select>
      ` : `
        <p>Choose from the render profiles supported by this source layout. Opacity changes the local map presentation; it does not edit the source or create a persistent style.</p>
        <label for="demo-source-profile">Render profile</label>
        <select id="demo-source-profile">${raster?.profiles.map((profile) => `<option value="${escapeAttribute(profile.id)}"${state.profile === profile.id ? ' selected' : ''}>${escape(profile.label)}</option>`).join('') ?? ''}</select>
        <label for="demo-source-opacity">Opacity <output data-field="opacity-value">${Math.round(state.opacity * 100)}%</output></label>
        <input id="demo-source-opacity" type="range" min="0" max="1" step="0.05" value="${state.opacity}" />
      `;
    body.innerHTML = `
      <section class="demo-source__configure" aria-labelledby="demo-source-configure-title">
        <p class="demo-source__section-label">03 / Map setup</p><h3 id="demo-source-configure-title">${vector ? 'Vector presentation' : rasterProfileHeading(raster?.layout)}</h3>
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
    body.querySelector<HTMLSelectElement>('#demo-source-profile')?.addEventListener('change', (event) => {
      this.#state = setDemoRasterProfile(this.#state, (event.currentTarget as HTMLSelectElement).value);
    });
    this.#attachActions(body);
  }

  #renderMap(body: HTMLElement): void {
    const state = this.#state as MappedDemoWorkflow;
    const source = state.source;
    body.innerHTML = `<section class="demo-source__map-ready" aria-labelledby="demo-source-map-title"><p class="demo-source__section-label">04 / Map</p><h3 id="demo-source-map-title">Temporary layer opened in the catalog map.</h3>${state.provenance ? provenanceMarkup(state.provenance) : ''}<p>${escape(source.attribution)}</p><p>The layer uses the server-advertised, same-origin tile link and will disappear with this source session.</p>${this.#discardError ? `<p class="demo-source__status demo-source__status--error" role="status">${escape(this.#discardError)}</p>` : ''}<button type="button" class="demo-source__quiet" data-action="reset"${this.#discarding ? ' disabled' : ''}>${this.#discarding ? 'Removing…' : 'Remove temporary layer'}</button></section>`;
    this.#attachActions(body);
  }

  #attachActions(body: HTMLElement): void {
    body.querySelector<HTMLButtonElement>('[data-action="configure"]')?.addEventListener('click', () => {
      this.#state = configureDemoSource(this.#state);
      this.#workflowStatus = '';
      this.#render();
    });
    body.querySelector<HTMLButtonElement>('[data-action="map"]')?.addEventListener('click', () => {
      const mapState = publishDemoMap(this.#state) as MappedDemoWorkflow;
      this.#state = mapState;
      this.#workflowStatus = '';
      const source = sourceForDemoRasterProfile(mapState.source, mapState.profile);
      this.dispatchEvent(new CustomEvent('tellurion-demo-map', { bubbles: true, composed: true, detail: { source, opacity: mapState.opacity, style: mapState.style, profile: mapState.profile } }));
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
      this.#workflowStatus = '';
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
    this.#workflowStatus = '';
    this.#state = resetDemoWorkflow(this.#state);
    if (this.#alive) {
      this.#render();
      this.querySelector<HTMLInputElement>('#demo-source-url')?.focus();
    }
  }
}

function extentText(
  extent: [number, number, number, number] | null,
  epsg: number | null,
): string {
  return extent ? `${extent.join(', ')} (${epsg === null ? 'reported CRS' : `EPSG:${epsg}`})` : 'Not reported';
}

function rasterProfileHeading(layout: string | undefined): string {
  if (layout === 'palette') return 'Embedded palette';
  if (layout === 'gray') return 'Grayscale rendering';
  return 'Native color rendering';
}

function rasterNodataText(nodata: NonNullable<InspectedDemoWorkflow['source']['raster']>['nodata']): string {
  if (nodata.state === 'value') return String(nodata.value);
  return nodata.state === 'unsupported' ? 'Declared, unsupported' : 'Not declared';
}

function formatRasterNumber(value: number): string {
  return value.toLocaleString(undefined, { maximumSignificantDigits: 8 });
}

function legendColor(rgba: [number, number, number, number]): string {
  const [red, green, blue, alpha] = rgba;
  return `rgb(${red} ${green} ${blue} / ${(alpha / 255).toFixed(3)})`;
}

function provenanceMarkup(provenance: DemoSourceProvenance): string {
  const identity = [
    provenance.collectionId ? `Collection ${provenance.collectionId}` : null,
    `Item ${provenance.itemId}`,
    `Asset ${provenance.assetKey}`,
  ].filter(Boolean).join(' · ');
  const metadata = [provenance.mediaType, provenance.roles.length ? `roles: ${provenance.roles.join(', ')}` : null]
    .filter(Boolean).join(' · ');
  const itemLink = provenance.itemHref
    ? `<p><a href="${escapeAttribute(provenance.itemHref)}" target="_blank" rel="noopener noreferrer">Open source STAC Item</a></p>`
    : '';
  const statement = provenance.assetDescription
    ? `<p><strong>Asset description (catalog supplied):</strong> ${escape(provenance.assetDescription)}</p>`
    : '';
  return `<aside class="demo-source__provenance" aria-label="STAC source context"><h4>STAC source context</h4><p>${escape(identity)}</p><p>${escape(provenance.assetLabel)}${metadata ? ` · ${escape(metadata)}` : ''}</p>${itemLink}${statement}</aside>`;
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
