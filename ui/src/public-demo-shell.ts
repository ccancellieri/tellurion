/** Mounts the intentionally narrow browser surface shipped with the public
 * demo image. The workflow and map viewer are registered by the entrypoint. */
export function mountPublicDemoShell(root: HTMLElement): void {
  root.replaceChildren(
    Object.assign(document.createElement('header'), {
      className: 'app-header',
      innerHTML: `
        <div>
          <p class="app-header__eyebrow">Tellurion public preview</p>
          <h1>Open a remote map where it already lives.</h1>
          <p>Enter a public HTTPS resource, inspect it through bounded byte-range reads, and view its short-lived layer without creating a tenant or catalog.</p>
        </div>
      `,
    }),
    Object.assign(document.createElement('main'), { className: 'public-demo-shell' }),
  );
  const main = root.querySelector('main');
  if (!main) throw new Error('public demo shell is missing its main region');
  main.append(
    document.createElement('tellurion-demo-source-workflow'),
    document.createElement('tellurion-demo-map-viewer'),
  );
}
