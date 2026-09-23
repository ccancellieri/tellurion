import { thirdPartyNoticesLink } from './legal';

/** Mounts the intentionally narrow browser surface shipped with the public
 * demo image. The workflow and map viewer are registered by the entrypoint. */
export function mountPublicDemoShell(root: HTMLElement): void {
  root.replaceChildren(
    Object.assign(document.createElement('header'), {
      className: 'app-header public-demo-header',
      innerHTML: `
        <div>
          <p class="app-header__eyebrow">Tellurion public preview</p>
          <h1>Your data. On the map.</h1>
          <p>Explore a public COG, GeoParquet or zipped Shapefile. Choose an example or paste a supported HTTPS address to get started.</p>
        </div>
      `,
    }),
    Object.assign(document.createElement('main'), { className: 'public-demo-shell' }),
    thirdPartyNoticesLink(),
  );
  const main = root.querySelector('main');
  if (!main) throw new Error('public demo shell is missing its main region');
  const resources = document.createElement('section');
  resources.className = 'public-demo-shell__resources';
  resources.setAttribute('aria-labelledby', 'public-demo-resources-title');
  resources.innerHTML = `
    <div class="public-demo-shell__resources-heading">
      <h2 id="public-demo-resources-title">Continue from the field desk</h2>
      <p>Try a source here, then use the project materials that match your next question.</p>
    </div>
    <nav aria-label="Tellurion resources">
      <ul>
        <li>
          <span class="public-demo-shell__resource-mark public-demo-shell__resource-mark--source" aria-hidden="true"></span>
          <a href="https://ccancellieri.github.io/tellurion/releases/v0.5.0/" target="_blank" rel="noopener noreferrer">Install Tellurion</a>
        </li>
        <li>
          <span class="public-demo-shell__resource-mark public-demo-shell__resource-mark--demo" aria-hidden="true"></span>
          <a href="https://ccancellieri.github.io/tellurion/" target="_blank" rel="noopener noreferrer">Verified demos</a>
        </li>
        <li>
          <span class="public-demo-shell__resource-mark public-demo-shell__resource-mark--feedback" aria-hidden="true"></span>
          <a href="https://github.com/ccancellieri/tellurion/issues/new?template=evaluation.yml" target="_blank" rel="noopener noreferrer">Share evaluation feedback</a>
        </li>
      </ul>
    </nav>
    <p class="public-demo-shell__boundary"><strong>Public HTTPS resources only.</strong> Temporary layers expire within 15 minutes. No account, upload, persistent tenant, or catalog is created.</p>
  `;
  main.append(
    document.createElement('tellurion-demo-source-workflow'),
    document.createElement('tellurion-demo-map-viewer'),
    resources,
  );
}
