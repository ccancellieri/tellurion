import maplibregl from 'maplibre-gl';
import '../operator-workspace.css';
import {
  DEFAULT_CATALOG_ID,
  DEFAULT_TENANT_ID,
  fetchCollections,
  fetchFeaturesLanding,
  fetchItems,
  fetchTenantDirectory,
  fetchTileSet,
  fetchTileSetList,
  type CollectionSummary,
  type FeatureCollectionResponse,
  type TileSet,
} from '../lib/api';
import {
  addWorkspaceLayer,
  catalogChoicesFromTenantLinks,
  contextFromSearch,
  contextSearch,
  createMap,
  demoRasterMapHandoff,
  fitToExtent,
  linkByRel,
  mergeCollectionPage,
  removeWorkspaceLayer,
  safeEndpointHref,
  selectCatalog,
  setWorkspaceLayerVisibility,
  withoutWorkspacePreview,
  workspaceMapIds,
  workspaceRenderPlan,
  type CatalogChoice,
  type WorkspaceContext,
  type WorkspaceLayer,
  type WorkspaceRenderPlan,
} from '../lib/map';
import type { DemoSourceResponse } from '../lib/demo-source';
import { resolveDataHref } from '../lib/tile-url';

interface WorkspaceCollection extends CollectionSummary, WorkspaceLayer {
  documentHref: string;
}

interface MapRegistration {
  sourceId: string;
  layerIds: string[];
}

interface TileSetRegistration {
  tileSet: TileSet;
  documentHref: string;
}

const VECTOR_PALETTE = [
  { fill: '#227c9d', line: '#11566f', point: '#e76f51' },
  { fill: '#4f7d4a', line: '#315d37', point: '#d89134' },
  { fill: '#725ca8', line: '#4f3f7f', point: '#d46b8c' },
  { fill: '#a06135', line: '#744326', point: '#3d8b8f' },
] as const;

/** A map-first catalog browser. Workspace layers are browser-local references;
 * choosing or removing one never mutates the catalog or copies its data. */
export class TellurionOperatorWorkspace extends HTMLElement {
  #map: maplibregl.Map | null = null;
  #collections: WorkspaceCollection[] = [];
  #layers: WorkspaceLayer[] = [];
  #previewFeatures = new Map<string, GeoJSON.Feature[]>();
  #plans = new Map<string, WorkspaceRenderPlan>();
  #tileSets = new Map<string, TileSetRegistration>();
  #registrations = new Map<string, MapRegistration>();
  #demoRegistration: MapRegistration | null = null;
  #pendingDemo: { source: DemoSourceResponse; opacity: number } | null = null;
  #pendingAdds = new Set<string>();
  #runtimeFallbacks = new Set<string>();
  #featureInspections = new Map<string, FeatureCollectionResponse>();
  #selectedId: string | null = null;
  #nextCollectionsHref: string | null = null;
  #activeContext: WorkspaceContext = {
    tenantId: DEFAULT_TENANT_ID,
    catalogId: DEFAULT_CATALOG_ID,
  };
  #alive = false;
  #browser!: HTMLElement;
  #details!: HTMLElement;
  #layerList!: HTMLElement;
  #status!: HTMLElement;
  #context!: HTMLElement;
  #tenantInput!: HTMLInputElement;
  #catalogSelect!: HTMLSelectElement;
  #demoMapListener = (event: Event): void => this.#receiveDemoMap(event);
  #demoResetListener = (event: Event): void => this.#receiveDemoReset(event);

  connectedCallback(): void {
    this.#alive = true;
    document.addEventListener('tellurion-demo-map', this.#demoMapListener);
    document.addEventListener('tellurion-demo-map-reset', this.#demoResetListener);
    const requestedContext = contextFromSearch(
      location.search,
      DEFAULT_TENANT_ID,
      DEFAULT_CATALOG_ID,
    );
    this.innerHTML = `
      <div class="workspace" data-state="loading">
        <aside class="workspace__sidebar" aria-label="Catalog workspace">
          <section class="workspace__context" aria-labelledby="workspace-context-title">
            <p class="workspace__eyebrow">Workspace</p>
            <h2 id="workspace-context-title" data-field="workspace-context-title">Open a catalog</h2>
            <form class="workspace__context-form" method="get" action="${this.#escapeAttribute(location.pathname)}" data-field="context-form">
              <label>Tenant<input name="tenant" data-field="tenant" value="${this.#escapeAttribute(requestedContext.tenantId)}" autocomplete="off" /></label>
              <label>Catalog<select name="catalog" data-field="catalog" disabled><option>Loading…</option></select></label>
              <button type="submit">Open catalog</button>
            </form>
            <p class="workspace__context-note" data-field="context"></p>
          </section>

          <section class="workspace__section" aria-labelledby="collection-browser-title">
            <div class="workspace__section-heading">
              <div>
                <p class="workspace__eyebrow">Catalog</p>
                <h2 id="collection-browser-title">Collections</h2>
              </div>
              <span class="workspace__count" data-field="count" aria-live="polite" tabindex="-1">Loading</span>
            </div>
            <p class="workspace__status" data-field="status" role="status">Loading tenant directory…</p>
            <div class="workspace__browser" data-field="browser" aria-live="polite"></div>
          </section>

          <section class="workspace__section" aria-labelledby="workspace-layers-title">
            <div class="workspace__section-heading">
              <div>
                <p class="workspace__eyebrow">Transient map</p>
                <h2 id="workspace-layers-title">Layers</h2>
              </div>
            </div>
            <p class="workspace__empty" data-field="layers-empty">Add a collection to view it here.</p>
            <ul class="workspace__layer-list" data-field="layers"></ul>
          </section>
        </aside>

        <section class="workspace__map-area" aria-label="Map workspace">
          <div class="workspace__map" data-field="map" aria-label="Collection map"></div>
          <p class="workspace__attribution">Data served by Tellurion. Basemap intentionally omitted.</p>
          <aside class="workspace__details" data-field="details" aria-live="polite">
            <p>Select a collection to inspect its advertised capabilities.</p>
          </aside>
        </section>
      </div>
    `;

    this.#browser = this.#field('browser');
    this.#details = this.#field('details');
    this.#layerList = this.#field('layers');
    this.#status = this.#field('status');
    this.#context = this.#field('context');
    this.#tenantInput = this.#field('tenant') as HTMLInputElement;
    this.#catalogSelect = this.#field('catalog') as HTMLSelectElement;
    this.#field('context-form').addEventListener('submit', () => {
      if (this.#tenantInput.value.trim() !== this.#activeContext.tenantId) {
        this.#catalogSelect.disabled = true;
      }
    });

    this.#map = createMap(this.#field('map'));
    this.#map.on('load', () => {
      this.#renderLayersOnMap();
      if (this.#pendingDemo) this.#openDemoMap(this.#pendingDemo.source, this.#pendingDemo.opacity);
    });
    this.#map.on('sourcedataloading', (event) => {
      if (event.sourceId && this.#collectionIdForSource(event.sourceId)) {
        this.#setStatus('Loading map data…');
      }
    });
    this.#map.on('sourcedata', (event) => {
      const collectionId = event.sourceId ? this.#collectionIdForSource(event.sourceId) : null;
      if (collectionId && event.isSourceLoaded) {
        const collection = this.#collection(collectionId);
        if (collection) this.#setStatus(`Map data ready for ${collection.title}.`);
      }
    });
    this.#map.on('error', (event) => {
      const sourceId = (event as typeof event & { sourceId?: unknown }).sourceId;
      const collectionId =
        typeof sourceId === 'string' ? this.#collectionIdForSource(sourceId) : null;
      if (collectionId) {
        void this.#fallbackAfterMapError(collectionId);
        return;
      }
      const message =
        event.error instanceof Error ? event.error.message : 'Map data could not be loaded.';
      this.#setStatus(message, true);
    });

    void this.#loadCatalog(requestedContext);
  }

  disconnectedCallback(): void {
    this.#alive = false;
    document.removeEventListener('tellurion-demo-map', this.#demoMapListener);
    document.removeEventListener('tellurion-demo-map-reset', this.#demoResetListener);
    this.#pendingAdds.clear();
    this.#map?.remove();
    this.#map = null;
  }

  #receiveDemoMap(event: Event): void {
    if (!(event instanceof CustomEvent)) return;
    const detail = event.detail;
    if (!detail || typeof detail !== 'object') return;
    const source = (detail as { source?: unknown }).source;
    const opacity = (detail as { opacity?: unknown }).opacity;
    if (!isDemoSourceResponse(source) || typeof opacity !== 'number' || !Number.isFinite(opacity)) return;
    this.#pendingDemo = { source, opacity: Math.max(0, Math.min(1, opacity)) };
    this.#openDemoMap(source, this.#pendingDemo.opacity);
  }

  #receiveDemoReset(event: Event): void {
    if (!(event instanceof CustomEvent)) return;
    const sourceId = (event.detail as { sourceId?: unknown } | null)?.sourceId;
    if (typeof sourceId !== 'string') return;
    if (this.#pendingDemo?.source.id === sourceId) this.#pendingDemo = null;
    this.#removeDemoMap(sourceId);
  }

  #openDemoMap(source: DemoSourceResponse, opacity: number): void {
    const map = this.#map;
    const handoff = demoRasterMapHandoff(source, location.origin);
    if (!map || !map.isStyleLoaded() || !handoff) return;
    this.#removeDemoMap();
    map.addSource(handoff.sourceId, {
      type: 'raster',
      tiles: [handoff.template],
      tileSize: 256,
      minzoom: 0,
      maxzoom: 22,
    });
    map.addLayer({
      id: handoff.layerId,
      type: 'raster',
      source: handoff.sourceId,
      paint: { 'raster-opacity': opacity },
    });
    this.#demoRegistration = { sourceId: handoff.sourceId, layerIds: [handoff.layerId] };
    if (handoff.extent) {
      fitToExtent(map, { spatial: { bbox: [handoff.extent], crs: 'EPSG:4326' } });
    }
    const attribution = this.querySelector<HTMLElement>('.workspace__attribution');
    if (attribution) attribution.textContent = `Temporary source: ${handoff.attribution}`;
    this.#setStatus('Temporary source map opened. It expires with this browser session.');
  }

  #removeDemoMap(sourceId?: string): void {
    const registration = this.#demoRegistration;
    if (!registration || (sourceId && registration.sourceId !== `demo-source-${sourceId}`)) return;
    const map = this.#map;
    if (map) {
      for (const layerId of registration.layerIds) {
        if (map.getLayer(layerId)) map.removeLayer(layerId);
      }
      if (map.getSource(registration.sourceId)) map.removeSource(registration.sourceId);
    }
    this.#demoRegistration = null;
    const attribution = this.querySelector<HTMLElement>('.workspace__attribution');
    if (attribution) attribution.textContent = 'Data served by Tellurion. Basemap intentionally omitted.';
  }

  async #loadCatalog(requested: WorkspaceContext): Promise<void> {
    try {
      const tenantDocumentHref = `/${encodeURIComponent(requested.tenantId)}`;
      const directory = await fetchTenantDirectory(requested.tenantId);
      if (!this.#alive) return;
      const choices = catalogChoicesFromTenantLinks(
        directory.links,
        tenantDocumentHref,
        location.origin,
        requested.tenantId,
      );
      const selection = selectCatalog(choices, requested.catalogId, requested.tenantId);
      this.#renderCatalogChoices(choices, selection.choice);
      if (!selection.choice) {
        this.#field('count').textContent = 'Unavailable';
        this.#setStatus(selection.reason ?? 'No Features catalog is available.', true);
        return;
      }

      this.#activeContext = {
        tenantId: requested.tenantId,
        catalogId: selection.choice.id,
      };
      this.#context.textContent = `Connected to ${this.#activeContext.tenantId}/${this.#activeContext.catalogId}.`;
      this.#field('workspace-context-title').textContent = this.#activeContext.catalogId;
      if (selection.reason) {
        history.replaceState(
          null,
          '',
          `${location.pathname}${contextSearch(this.#activeContext)}${location.hash}`,
        );
      }

      const landing = await fetchFeaturesLanding(selection.choice.featuresRoot);
      if (!this.#alive) return;
      const dataLink = linkByRel(landing.links, 'data');
      const collectionsHref = dataLink
        ? resolveDataHref(dataLink.href, selection.choice.featuresRoot, location.origin)
        : null;
      if (!collectionsHref) throw new Error('The Features root does not advertise a safe collections link.');
      await this.#loadCollectionPage(collectionsHref, false);
      if (selection.reason && this.#alive) this.#setStatus(selection.reason);
    } catch (error) {
      if (!this.#alive) return;
      this.#field('count').textContent = 'Unavailable';
      this.#setStatus(error instanceof Error ? error.message : String(error), true);
      this.#browser.innerHTML = '<p class="workspace__empty">Collections could not be loaded.</p>';
    }
  }

  #renderCatalogChoices(choices: readonly CatalogChoice[], selected: CatalogChoice | null): void {
    const options = choices.map((choice) => {
      const option = document.createElement('option');
      option.value = choice.id;
      option.textContent = choice.id;
      option.selected = choice.id === selected?.id;
      return option;
    });
    this.#catalogSelect.replaceChildren(...options);
    this.#catalogSelect.disabled = choices.length === 0;
  }

  async #loadCollectionPage(
    href: string,
    append: boolean,
    restoreFocus = false,
  ): Promise<void> {
    try {
      if (append) this.#setStatus('Loading more collections…');
      else this.#setStatus('Loading collections…');
      const response = await fetchCollections(href);
      if (!this.#alive) return;
      const pageCollections: WorkspaceCollection[] = response.collections.map((collection) => {
        const selfLink = linkByRel(collection.links, 'self');
        const documentHref = selfLink
          ? resolveDataHref(selfLink.href, href, location.origin) ?? href
          : href;
        return {
          ...collection,
          documentHref,
          title: collection.title || collection.id,
          visible: true,
        };
      });
      const merged = mergeCollectionPage(
        append ? this.#collections : [],
        { collections: pageCollections, links: response.links },
        href,
        location.origin,
      );
      this.#collections = merged.collections;
      this.#nextCollectionsHref = merged.nextHref;
      this.#renderBrowser();
      this.#field('count').textContent = `${this.#collections.length} loaded${this.#nextCollectionsHref ? ' · more available' : ''}`;
      if (this.#collections.length === 0) {
        this.#setStatus('No collections are available in this catalog.');
        return;
      }
      this.#setStatus('Select a collection to inspect it, then add it to the map.');
      if (!this.#selectedId) this.#selectCollection(this.#collections[0].id);
      if (restoreFocus) this.#focusAfterCollectionPage();
    } catch (error) {
      if (!this.#alive) return;
      this.#setStatus(error instanceof Error ? error.message : String(error), true);
      if (!append) {
        this.#browser.innerHTML = '<p class="workspace__empty">Collections could not be loaded.</p>';
      } else {
        this.#renderBrowser();
        if (restoreFocus) this.#focusAfterCollectionPage();
      }
    }
  }

  #renderBrowser(): void {
    if (this.#collections.length === 0) {
      this.#browser.innerHTML = '<p class="workspace__empty">No collections have been registered yet.</p>';
      return;
    }

    const list = document.createElement('ul');
    list.className = 'workspace__collection-list';
    for (const collection of this.#collections) {
      const item = document.createElement('li');
      const select = document.createElement('button');
      select.type = 'button';
      select.className = 'workspace__collection-select';
      select.dataset.collectionId = collection.id;
      select.setAttribute('aria-pressed', String(this.#selectedId === collection.id));
      select.innerHTML = `<strong>${this.#escape(collection.title)}</strong><span>${this.#escape(collection.id)}</span>`;
      select.addEventListener('click', () => this.#selectCollection(collection.id));
      item.append(select);
      list.append(item);
    }
    const children: HTMLElement[] = [list];
    if (this.#nextCollectionsHref) {
      const loadMore = document.createElement('button');
      loadMore.type = 'button';
      loadMore.className = 'workspace__load-more';
      loadMore.textContent = 'Load more collections';
      loadMore.addEventListener('click', () => {
        const nextHref = this.#nextCollectionsHref;
        if (nextHref) void this.#loadCollectionPage(nextHref, true, true);
      });
      children.push(loadMore);
    }
    this.#browser.replaceChildren(...children);
  }

  #selectCollection(id: string): void {
    const collection = this.#collection(id);
    if (!collection) return;
    this.#selectedId = id;
    this.#browser
      .querySelectorAll<HTMLButtonElement>('[data-collection-id]')
      .forEach((button) =>
        button.setAttribute('aria-pressed', String(button.dataset.collectionId === id)),
      );
    this.#renderDetails(collection);
  }

  #renderDetails(collection: WorkspaceCollection): void {
    const inWorkspace = this.#layers.some((layer) => layer.id === collection.id);
    const pending = this.#pendingAdds.has(collection.id);
    const itemsLink = linkByRel(collection.links, 'items');
    const itemsHref = itemsLink
      ? resolveDataHref(itemsLink.href, collection.documentHref, location.origin)
      : null;
    const plan = this.#plans.get(this.#planKey(collection.id));
    const unavailable = plan?.mode === 'unavailable';
    const capabilities = collection.links.map((link) => link.rel.split('/').pop() ?? link.rel);
    const endpointList = collection.links
      .map((link) => {
        const href = safeEndpointHref(link.href, location.origin);
        const label = this.#escape(link.rel || link.type || 'endpoint');
        return href
          ? `<li><a href="${this.#escapeAttribute(href)}" target="_blank" rel="noreferrer">${label}</a></li>`
          : `<li><span>${label} (unsupported link)</span></li>`;
      })
      .join('');
    const extent = collection.extent?.spatial.bbox[0];
    const extentText = extent
      ? extent.map((coordinate) => coordinate.toFixed(3)).join(', ')
      : 'Not reported';
    const planText = plan ? this.#planDescription(plan) : 'Resolved when the layer is added';
    const inspection = this.#featureInspections.get(collection.id);
    this.#details.innerHTML = `
      <p class="workspace__eyebrow">Collection</p>
      <h2 tabindex="-1">${this.#escape(collection.title)}</h2>
      <p class="workspace__collection-id">${this.#escape(collection.id)}</p>
      <dl class="workspace__metadata">
        <div><dt>Type</dt><dd>${this.#escape(collection.itemType || 'Not reported')}</dd></div>
        <div><dt>Extent</dt><dd>${this.#escape(extentText)}</dd></div>
        <div><dt>Capabilities</dt><dd>${this.#escape(capabilities.join(', ') || 'Not reported')}</dd></div>
        <div><dt>Map representation</dt><dd>${this.#escape(planText)}</dd></div>
      </dl>
      ${plan?.reason ? `<p class="workspace__fallback">${this.#escape(plan.reason)}</p>` : ''}
      <div class="workspace__detail-actions">
        <button type="button" data-action="add" ${inWorkspace || pending || unavailable ? 'disabled' : ''}>${
          pending ? 'Discovering…' : unavailable ? 'Unavailable' : inWorkspace ? 'Already in map' : 'Add to map'
        }</button>
        <button type="button" data-action="zoom" ${extent ? '' : 'disabled'}>${extent ? 'Zoom to extent' : 'Extent unavailable'}</button>
        ${itemsHref ? '<button type="button" data-action="preview">Inspect first page</button>' : ''}
      </div>
      ${inspection ? this.#inspectionHtml(inspection) : ''}
      <details class="workspace__endpoints">
        <summary>Available endpoints</summary>
        ${endpointList ? `<ul>${endpointList}</ul>` : '<p>No endpoints were advertised.</p>'}
      </details>
      <p class="workspace__help">Tellurion follows the collection’s advertised capabilities and prefers vector tiles, then raster tiles, then one bounded feature page. Adding a layer is temporary and never copies or changes the data.</p>
    `;
    this.#details.querySelector<HTMLButtonElement>('[data-action="add"]')?.addEventListener('click', () =>
      void this.#addCollection(collection),
    );
    this.#details.querySelector<HTMLButtonElement>('[data-action="zoom"]')?.addEventListener('click', () =>
      this.#zoomTo(collection),
    );
    this.#details.querySelector<HTMLButtonElement>('[data-action="preview"]')?.addEventListener('click', () =>
      void this.#previewFeaturePage(collection, itemsHref!),
    );
  }

  async #addCollection(collection: WorkspaceCollection): Promise<void> {
    if (
      this.#pendingAdds.has(collection.id) ||
      this.#layers.some((layer) => layer.id === collection.id)
    ) {
      return;
    }
    const restoreFocus = this.#details.contains(document.activeElement);
    this.#pendingAdds.add(collection.id);
    const addButton = this.#details.querySelector<HTMLButtonElement>('[data-action="add"]');
    if (addButton) {
      addButton.disabled = true;
      addButton.textContent = 'Discovering…';
    }
    try {
      this.#setStatus(`Discovering the best map representation for ${collection.title}…`);
      const plan = await this.#discoverPlan(collection);
      if (!this.#alive || !this.#pendingAdds.has(collection.id)) return;
      this.#plans.set(this.#planKey(collection.id), plan);
      if (plan.mode === 'preview') await this.#loadPreview(collection, plan.itemsHref);
      if (!this.#alive || !this.#pendingAdds.has(collection.id)) return;
      if (plan.mode === 'unavailable') {
        this.#layers = addWorkspaceLayer(this.#layers, collection);
        this.#renderLayerList();
        this.#setStatus(`${collection.title}: ${plan.reason}`, true);
        return;
      }
      this.#layers = addWorkspaceLayer(this.#layers, collection);
      this.#renderLayerList();
      this.#addCollectionToMap(collection);
      this.#zoomTo(collection);
      const description = this.#planDescription(plan);
      this.#setStatus(
        `${plan.reason ? `${plan.reason} ` : ''}Added ${collection.title} using ${description}.`,
      );
    } catch (error) {
      if (this.#alive) {
        this.#setStatus(
          error instanceof Error ? error.message : `Could not add ${collection.title}.`,
          true,
        );
      }
    } finally {
      this.#pendingAdds.delete(collection.id);
      if (this.#alive && this.#selectedId === collection.id) {
        this.#renderDetails(collection);
        if (restoreFocus) this.#details.querySelector<HTMLElement>('h2')?.focus();
      }
    }
  }

  async #discoverPlan(collection: WorkspaceCollection): Promise<WorkspaceRenderPlan> {
    const key = this.#planKey(collection.id);
    const cached = this.#plans.get(key);
    if (cached) return cached;
    const tilesLink =
      linkByRel(collection.links, 'tilesets-vector') ??
      linkByRel(collection.links, 'tilesets-map');
    if (!tilesLink) return this.#fallbackPlan(collection);
    const listHref = resolveDataHref(tilesLink.href, collection.documentHref, location.origin);
    if (!listHref) return this.#fallbackPlan(collection, 'The advertised TileSet link is unsafe.');

    try {
      const list = await fetchTileSetList(listHref);
      for (const summary of list.tilesets) {
        const selfLink = linkByRel(summary.links, 'self');
        const tileSetHref = selfLink
          ? resolveDataHref(selfLink.href, listHref, location.origin)
          : null;
        if (!tileSetHref) continue;
        const tileSet = await fetchTileSet(tileSetHref);
        const plan = this.#planForTileSet(collection, tileSet, tileSetHref);
        if (plan.mode === 'vector' || plan.mode === 'raster') {
          this.#tileSets.set(key, { tileSet, documentHref: tileSetHref });
          return plan;
        }
      }
      return this.#fallbackPlan(
        collection,
        'No compatible WebMercatorQuad TileSet was advertised; using the next available representation.',
      );
    } catch (error) {
      const reason = error instanceof Error ? error.message : 'TileSet discovery failed.';
      return this.#fallbackPlan(collection, `${reason} Using the next available representation.`);
    }
  }

  #planForTileSet(
    collection: WorkspaceCollection,
    tileSet: TileSet,
    tileSetHref: string,
  ): WorkspaceRenderPlan {
    return workspaceRenderPlan({
      collectionLinks: collection.links,
      collectionDocumentHref: collection.documentHref,
      tileSet,
      tileSetDocumentHref: tileSetHref,
      origin: location.origin,
    });
  }

  #fallbackPlan(collection: WorkspaceCollection, reason?: string): WorkspaceRenderPlan {
    return workspaceRenderPlan({
      collectionLinks: collection.links,
      collectionDocumentHref: collection.documentHref,
      tileSet: null,
      tileSetDocumentHref: collection.documentHref,
      origin: location.origin,
      fallbackReason: reason,
    });
  }

  async #fallbackAfterMapError(collectionId: string): Promise<void> {
    if (this.#runtimeFallbacks.has(collectionId)) return;
    const collection = this.#collection(collectionId);
    const key = this.#planKey(collectionId);
    const current = this.#plans.get(key);
    const registeredTileSet = this.#tileSets.get(key);
    if (
      !collection ||
      !registeredTileSet ||
      (current?.mode !== 'vector' && current?.mode !== 'raster')
    ) {
      return;
    }

    this.#runtimeFallbacks.add(collectionId);
    this.#removeCollectionFromMap(collectionId);
    const failedMode = current.mode;
    const reason = `${failedMode === 'vector' ? 'Vector' : 'Raster'} tile requests failed; using the next advertised representation.`;
    let next = workspaceRenderPlan({
      collectionLinks: collection.links,
      collectionDocumentHref: collection.documentHref,
      tileSet: registeredTileSet.tileSet,
      tileSetDocumentHref: registeredTileSet.documentHref,
      origin: location.origin,
      fallbackReason: reason,
      excludedModes: failedMode === 'vector' ? ['vector'] : ['vector', 'raster'],
    });

    try {
      if (next.mode === 'preview') await this.#loadPreview(collection, next.itemsHref);
    } catch {
      next = {
        mode: 'unavailable',
        reason: `${reason} The bounded feature preview also failed.`,
      };
    } finally {
      this.#runtimeFallbacks.delete(collectionId);
    }

    if (!this.#alive || !this.#layers.some((layer) => layer.id === collectionId)) return;
    this.#plans.set(key, next);
    this.#renderLayerList();
    if (next.mode !== 'unavailable') this.#addCollectionToMap(collection);
    this.#setStatus(
      next.mode === 'unavailable'
        ? `${collection.title}: ${next.reason}`
        : `${next.reason ?? reason} Switched ${collection.title} to ${this.#planDescription(next)}.`,
      next.mode === 'unavailable',
    );
    if (this.#selectedId === collectionId) this.#renderDetails(collection);
  }

  #renderLayerList(): void {
    const empty = this.#field('layers-empty');
    empty.hidden = this.#layers.length !== 0;
    this.#layerList.replaceChildren(
      ...this.#layers.map((layer) => {
        const collection = this.#collection(layer.id)!;
        const plan = this.#plans.get(this.#planKey(layer.id));
        const unavailable = plan?.mode === 'unavailable';
        const item = document.createElement('li');
        item.className = 'workspace__layer';
        item.innerHTML = `
          <label><input type="checkbox" ${layer.visible ? 'checked' : ''} ${unavailable ? 'disabled' : ''} /> <span>${this.#escape(layer.title)}</span></label>
          <p class="workspace__layer-mode">${this.#escape(plan ? this.#planDescription(plan) : 'Representation unavailable')}</p>
          <div>
            <button type="button" data-action="zoom">Zoom</button>
            <button type="button" data-action="remove">Remove</button>
          </div>
        `;
        item.querySelector<HTMLInputElement>('input')!.addEventListener('change', (event) => {
          const target = event.currentTarget as HTMLInputElement;
          this.#layers = setWorkspaceLayerVisibility(this.#layers, layer.id, target.checked);
          this.#setMapLayerVisibility(layer.id, target.checked);
        });
        item.querySelector<HTMLButtonElement>('[data-action="zoom"]')!.addEventListener('click', () =>
          this.#zoomTo(collection),
        );
        item.querySelector<HTMLButtonElement>('[data-action="remove"]')!.addEventListener('click', () =>
          this.#removeCollection(collection),
        );
        return item;
      }),
    );
  }

  #removeCollection(collection: WorkspaceCollection): void {
    this.#pendingAdds.delete(collection.id);
    this.#runtimeFallbacks.delete(collection.id);
    this.#layers = removeWorkspaceLayer(this.#layers, collection.id);
    this.#removeCollectionFromMap(collection.id);
    this.#previewFeatures = withoutWorkspacePreview(this.#previewFeatures, collection.id);
    this.#renderLayerList();
    this.#renderDetails(collection);
    this.#setStatus(`Removed ${collection.title} from this browser workspace.`);
  }

  async #previewFeaturePage(collection: WorkspaceCollection, href: string): Promise<void> {
    const restoreFocus = this.#details.contains(document.activeElement);
    try {
      this.#setStatus(`Loading a feature preview for ${collection.title}…`);
      const page = await fetchItems(href);
      if (!this.#alive) return;
      this.#featureInspections.set(collection.id, page);
      if (this.#selectedId === collection.id) {
        this.#renderDetails(collection);
        if (restoreFocus) {
          this.#details.querySelector<HTMLElement>('[data-field="feature-inspection"]')?.focus();
        }
      }
      const matched = page.numberMatched === undefined ? '' : ` of ${page.numberMatched} matched`;
      this.#setStatus(`${page.numberReturned} feature(s) returned${matched} for ${collection.title}.`);
    } catch (error) {
      if (!this.#alive) return;
      this.#setStatus(
        error instanceof Error ? error.message : `Feature preview failed: ${String(error)}`,
        true,
      );
    }
  }

  #inspectionHtml(page: FeatureCollectionResponse): string {
    const matched = page.numberMatched === undefined ? '' : ` of ${page.numberMatched} matched`;
    const rows = page.features.slice(0, 10).map((feature, index) => {
      const id = feature.id === undefined ? `Feature ${index + 1}` : String(feature.id);
      const geometry = feature.geometry?.type ?? 'No geometry';
      const fields = Object.keys(feature.properties ?? {}).slice(0, 5);
      return `<li><strong>${this.#escape(id)}</strong><span>${this.#escape(geometry)}${
        fields.length ? ` · ${this.#escape(fields.join(', '))}` : ''
      }</span></li>`;
    });
    return `
      <section class="workspace__inspection" data-field="feature-inspection" tabindex="-1" aria-labelledby="workspace-inspection-title">
        <h3 id="workspace-inspection-title">First feature page</h3>
        <p>${page.numberReturned} feature(s) returned${matched}.</p>
        ${rows.length ? `<ol>${rows.join('')}</ol>` : '<p>No features were returned.</p>'}
      </section>
    `;
  }

  #focusAfterCollectionPage(): void {
    const target =
      this.#browser.querySelector<HTMLElement>('.workspace__load-more') ??
      this.#field('count');
    target.focus();
  }

  #renderLayersOnMap(): void {
    for (const layer of this.#layers) {
      const collection = this.#collection(layer.id);
      if (collection) this.#addCollectionToMap(collection);
    }
  }

  #addCollectionToMap(collection: WorkspaceCollection): void {
    const map = this.#map;
    const plan = this.#plans.get(this.#planKey(collection.id));
    if (!map?.isStyleLoaded() || !plan || plan.mode === 'unavailable') return;
    if (this.#registrations.has(collection.id)) return;
    const ids = workspaceMapIds(collection.id, plan);
    if (!ids.sourceId) return;

    if (plan.mode === 'vector') {
      map.addSource(ids.sourceId, {
        type: 'vector',
        tiles: [plan.template],
        minzoom: plan.minzoom,
        maxzoom: plan.maxzoom,
      });
      plan.sourceLayers.forEach((sourceLayer, index) => {
        const palette = VECTOR_PALETTE[index % VECTOR_PALETTE.length];
        const [fillId, lineId, pointId] = ids.layerIds.slice(index * 3, index * 3 + 3);
        map.addLayer({
          id: fillId,
          type: 'fill',
          source: ids.sourceId!,
          'source-layer': sourceLayer,
          filter: ['==', ['geometry-type'], 'Polygon'],
          paint: {
            'fill-color': palette.fill,
            'fill-opacity': 0.35,
            'fill-outline-color': palette.line,
          },
        });
        map.addLayer({
          id: lineId,
          type: 'line',
          source: ids.sourceId!,
          'source-layer': sourceLayer,
          filter: ['==', ['geometry-type'], 'LineString'],
          paint: { 'line-color': palette.line, 'line-width': 2 },
        });
        map.addLayer({
          id: pointId,
          type: 'circle',
          source: ids.sourceId!,
          'source-layer': sourceLayer,
          filter: ['==', ['geometry-type'], 'Point'],
          paint: {
            'circle-color': palette.point,
            'circle-radius': 5,
            'circle-stroke-width': 1,
            'circle-stroke-color': '#fff',
          },
        });
      });
    } else if (plan.mode === 'raster') {
      map.addSource(ids.sourceId, {
        type: 'raster',
        tiles: [plan.template],
        tileSize: 256,
        minzoom: plan.minzoom,
        maxzoom: plan.maxzoom,
      });
      map.addLayer({ id: ids.layerIds[0], type: 'raster', source: ids.sourceId });
    } else {
      const features = this.#previewFeatures.get(collection.id);
      if (!features) return;
      map.addSource(ids.sourceId, {
        type: 'geojson',
        data: { type: 'FeatureCollection', features },
      });
      map.addLayer({
        id: ids.layerIds[0],
        type: 'fill',
        source: ids.sourceId,
        filter: ['==', ['geometry-type'], 'Polygon'],
        paint: { 'fill-color': '#227c9d', 'fill-opacity': 0.35, 'fill-outline-color': '#11566f' },
      });
      map.addLayer({
        id: ids.layerIds[1],
        type: 'line',
        source: ids.sourceId,
        filter: ['==', ['geometry-type'], 'LineString'],
        paint: { 'line-color': '#11566f', 'line-width': 2 },
      });
      map.addLayer({
        id: ids.layerIds[2],
        type: 'circle',
        source: ids.sourceId,
        filter: ['==', ['geometry-type'], 'Point'],
        paint: {
          'circle-color': '#e76f51',
          'circle-radius': 5,
          'circle-stroke-width': 1,
          'circle-stroke-color': '#fff',
        },
      });
    }
    this.#registrations.set(collection.id, { sourceId: ids.sourceId, layerIds: ids.layerIds });
    const layer = this.#layers.find((current) => current.id === collection.id);
    this.#setMapLayerVisibility(collection.id, layer?.visible ?? true);
  }

  #removeCollectionFromMap(collectionId: string): void {
    const map = this.#map;
    const registration = this.#registrations.get(collectionId);
    if (!map || !registration) return;
    for (const layerId of registration.layerIds) {
      if (map.getLayer(layerId)) map.removeLayer(layerId);
    }
    if (map.getSource(registration.sourceId)) map.removeSource(registration.sourceId);
    this.#registrations.delete(collectionId);
  }

  #setMapLayerVisibility(collectionId: string, visible: boolean): void {
    const map = this.#map;
    const registration = this.#registrations.get(collectionId);
    if (!map?.isStyleLoaded() || !registration) return;
    for (const layerId of registration.layerIds) {
      if (map.getLayer(layerId)) {
        map.setLayoutProperty(layerId, 'visibility', visible ? 'visible' : 'none');
      }
    }
  }

  #zoomTo(collection: WorkspaceCollection): void {
    if (this.#map) fitToExtent(this.#map, collection.extent);
  }

  #collection(id: string): WorkspaceCollection | undefined {
    return this.#collections.find((collection) => collection.id === id);
  }

  #collectionIdForSource(id: string): string | null {
    for (const [collectionId, registration] of this.#registrations) {
      if (registration.sourceId === id) return collectionId;
    }
    return null;
  }

  async #loadPreview(collection: WorkspaceCollection, href: string): Promise<void> {
    this.#setStatus(`Loading the first feature page for ${collection.title}…`);
    const page = await fetchItems(href);
    if (!this.#alive) return;
    this.#previewFeatures.set(collection.id, page.features);
    this.#setStatus(
      `Loaded ${page.numberReturned} feature(s) as a bounded preview for ${collection.title}; the full dataset remains on the server.`,
    );
  }

  #planKey(collectionId: string): string {
    return `${this.#activeContext.tenantId}/${this.#activeContext.catalogId}/${collectionId}`;
  }

  #planDescription(plan: WorkspaceRenderPlan): string {
    switch (plan.mode) {
      case 'vector':
        return `Vector tiles · ${plan.sourceLayers.join(', ')}`;
      case 'raster':
        return 'Raster tiles';
      case 'preview':
        return 'Bounded feature preview';
      case 'unavailable':
        return 'Unavailable';
    }
  }

  #setStatus(message: string, isError = false): void {
    this.#status.textContent = message;
    this.#status.classList.toggle('workspace__status--error', isError);
  }

  #field(name: string): HTMLElement {
    const element = this.querySelector<HTMLElement>(`[data-field="${name}"]`);
    if (!element) throw new Error(`operator workspace is missing its "${name}" field`);
    return element;
  }

  #escape(value: string): string {
    return value.replace(/[&<>"']/g, (character) => ({
      '&': '&amp;',
      '<': '&lt;',
      '>': '&gt;',
      '"': '&quot;',
      "'": '&#39;',
    })[character]!);
  }

  #escapeAttribute(value: string): string {
    return this.#escape(value);
  }
}

customElements.define('tellurion-operator-workspace', TellurionOperatorWorkspace);

function isDemoSourceResponse(value: unknown): value is DemoSourceResponse {
  if (!value || typeof value !== 'object') return false;
  const source = value as Partial<DemoSourceResponse>;
  return (
    typeof source.id === 'string' &&
    typeof source.format === 'string' &&
    typeof source.transport === 'string' &&
    typeof source.revision === 'string' &&
    typeof source.capability_state === 'string' &&
    typeof source.attribution === 'string' &&
    (source.extent === null ||
      (Array.isArray(source.extent) &&
        source.extent.length === 4 &&
        source.extent.every((coordinate) => typeof coordinate === 'number' && Number.isFinite(coordinate)))) &&
    !!source.links &&
    typeof source.links.self_href === 'string' &&
    typeof source.links.tile_template === 'string' &&
    !!source.limits &&
    typeof source.limits.expires_in_seconds === 'number'
  );
}
