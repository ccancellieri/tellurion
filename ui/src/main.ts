import 'maplibre-gl/dist/maplibre-gl.css';
import './style.css';
import './elements/demo-source-workflow';
import './elements/demo-map-viewer';
import { mountControlWorkspace, workspaceModeFor } from './elements/control-shell';
import { thirdPartyNoticesLink } from './legal';
import { mountPublicDemoShell } from './public-demo-shell';

function mountOperatorConsole(): void {

document.body.replaceChildren(
  Object.assign(document.createElement('header'), {
    className: 'app-header',
    innerHTML: `
      <div>
        <p class="app-header__eyebrow">Tellurion field console</p>
        <h1>Open a public source. Inspect it. Map it.</h1>
        <p>Try a bounded remote raster read first, then continue into the catalog workspace and its existing OGC capabilities.</p>
      </div>
      <tellurion-status-widget></tellurion-status-widget>
    `,
  }),
  document.createElement('main'),
  thirdPartyNoticesLink(),
);

const main = document.querySelector('main');
if (!main) throw new Error('application shell is missing its main region');
main.append(document.createElement('tellurion-demo-source-workflow'));
main.append(document.createElement('tellurion-operator-workspace'));

const lab = document.createElement('details');
lab.className = 'protocol-lab';
lab.innerHTML = `
  <summary>Advanced protocol lab</summary>
  <p>Explore individual protocol lanes without changing the catalog workspace.</p>
  <div class="protocol-lab__tabs" role="tablist" aria-label="Protocol lanes">
    <button type="button" role="tab" id="features-tab" aria-controls="features-panel" aria-selected="true">Features</button>
    <button type="button" role="tab" id="vector-tab" aria-controls="vector-panel" aria-selected="false" tabindex="-1">MVT</button>
    <button type="button" role="tab" id="png-tab" aria-controls="png-panel" aria-selected="false" tabindex="-1">PNG</button>
    <button type="button" role="tab" id="styled-tab" aria-controls="styled-panel" aria-selected="false" tabindex="-1">Styles</button>
    <button type="button" role="tab" id="places3d-tab" aria-controls="places3d-panel" aria-selected="false" tabindex="-1">3D</button>
  </div>
  <p class="protocol-lab__status" data-field="protocol-status" role="status"></p>
  <section id="features-panel" role="tabpanel" aria-labelledby="features-tab"></section>
  <section id="vector-panel" role="tabpanel" aria-labelledby="vector-tab" hidden>
    <p class="protocol-lab__note">MVT is a diagnostic lane: a collection’s advertised links may not identify a stable source-layer name, external ID, or PMTiles source.</p>
  </section>
  <section id="png-panel" role="tabpanel" aria-labelledby="png-tab" hidden></section>
  <section id="styled-panel" role="tabpanel" aria-labelledby="styled-tab" hidden></section>
  <section id="places3d-panel" role="tabpanel" aria-labelledby="places3d-tab" hidden></section>
`;
main.append(lab);

const protocolTabs = Array.from(lab.querySelectorAll<HTMLButtonElement>('[role="tab"]'));
const mountedProtocolPanels = new Set<string>();
const loadingProtocolPanels = new Map<string, Promise<void>>();
const protocolPanelTags: Record<string, string> = {
  features: 'tellurion-features-panel',
  vector: 'tellurion-vector-panel',
  png: 'tellurion-png-panel',
  styled: 'tellurion-styled-panel',
  places3d: 'tellurion-places3d-panel',
};

function setProtocolStatus(message: string, retryTabId?: string): void {
  const status = lab.querySelector<HTMLElement>('[data-field="protocol-status"]');
  if (!status) throw new Error('protocol lab is missing its status region');
  status.replaceChildren(document.createTextNode(message));
  if (retryTabId) {
    const retry = document.createElement('button');
    retry.type = 'button';
    retry.textContent = 'Retry';
    retry.addEventListener('click', () => void activateProtocolPanel(retryTabId));
    status.append(' ', retry);
  }
}

async function importProtocolPanel(tabId: string): Promise<void> {
  if (mountedProtocolPanels.has(tabId)) return;
  const pending = loadingProtocolPanels.get(tabId);
  if (pending) return pending;
  const load = (async (): Promise<void> => {
    setProtocolStatus(`Loading ${tabId} protocol tools…`);
    try {
      if (tabId === 'features') await import('./elements/features-panel');
      if (tabId === 'vector') await import('./elements/vector-tiles-panel');
      if (tabId === 'png') await import('./elements/png-tiles-panel');
      if (tabId === 'styled') await import('./elements/styled-panel');
      if (tabId === 'places3d') await import('./elements/places3d-panel');

      const panel = lab.querySelector<HTMLElement>(`#${tabId}-panel`);
      const tag = protocolPanelTags[tabId];
      if (!panel || !tag) throw new Error(`protocol lab cannot mount ${tabId}`);
      panel.append(document.createElement(tag));
      mountedProtocolPanels.add(tabId);
      setProtocolStatus('');
    } catch (error) {
      setProtocolStatus(
        `Could not load ${tabId} protocol tools: ${error instanceof Error ? error.message : String(error)}.`,
        tabId,
      );
    } finally {
      loadingProtocolPanels.delete(tabId);
    }
  })();
  loadingProtocolPanels.set(tabId, load);
  await load;
}

async function activateProtocolPanel(tabId: string): Promise<void> {
  protocolTabs.forEach((candidate) => {
    const active = candidate.id === `${tabId}-tab`;
    candidate.setAttribute('aria-selected', String(active));
    candidate.tabIndex = active ? 0 : -1;
    const panel = lab.querySelector<HTMLElement>(`#${candidate.getAttribute('aria-controls')}`);
    if (panel) panel.hidden = !active;
  });
  await importProtocolPanel(tabId);
}

protocolTabs.forEach((tab, index) => {
  const tabId = tab.id.replace('-tab', '');
  tab.addEventListener('click', () => void activateProtocolPanel(tabId));
  tab.addEventListener('keydown', (event) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const nextIndex =
      event.key === 'Home'
        ? 0
        : event.key === 'End'
          ? protocolTabs.length - 1
          : (index + (event.key === 'ArrowRight' ? 1 : -1) + protocolTabs.length) % protocolTabs.length;
    const next = protocolTabs[nextIndex];
    void activateProtocolPanel(next.id.replace('-tab', ''));
    next.focus();
  });
});

lab.addEventListener('toggle', () => {
  if (lab.open) void activateProtocolPanel('features');
});
}

const controlMode = workspaceModeFor(location.pathname, import.meta.env.MODE);

if (controlMode) {
  mountControlWorkspace(document.body, controlMode);
  document.body.append(thirdPartyNoticesLink());
} else if (import.meta.env.MODE === 'public-demo') {
  mountPublicDemoShell(document.body);
} else {
  void Promise.all([
    import('./elements/status-widget'),
    import('./elements/operator-workspace'),
  ]).then(() => mountOperatorConsole());
}
