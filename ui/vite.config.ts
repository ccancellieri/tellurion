import { defineConfig } from 'vite';

// Dev-only convenience: the demo panels call the Tellurion API with
// root-absolute paths under the tenant-scoped route tree (e.g.
// `/public/features/catalogs/default/collections` — see `lib/api.ts`'s
// `DEFAULT_TENANT_ID`/`DEFAULT_CATALOG_ID`) plus the top-level `/metrics`
// endpoint, so the same code works whether the bundle is served standalone
// or embedded by the server at `/ui`. In `vite dev`, though, those paths
// would otherwise hit the Vite dev server itself, which doesn't speak the
// API — proxy them through to a real `tellurion` instance. `/public` as a
// prefix covers every protocol root nested under it (features/tiles/styles/
// 3dtiles, and each root's own `/conformance`/`/api`). The control workspace
// and anonymous source inspector have separate top-level roots, so they must
// follow the same selected application origin too. Not used by `vite build`.
const API_ROUTES = ['/public', '/metrics', '/_control', '/_auth', '/demo'];

export default defineConfig(() => ({
  // Relative asset paths: the exact same `ui/dist` bundle then works
  // whether it's embedded by the server at `/ui/` or hosted standalone at
  // the root of any static file server — an absolute `/ui/` base would
  // break the latter. Relative paths only resolve correctly against a
  // document URL that ends in `/`, which is why the server redirects bare
  // `/ui` to `/ui/` before serving the shell (see `ui_assets.rs`).
  base: './',
  server: {
    proxy: Object.fromEntries(
      API_ROUTES.map((path) => [
        path,
        process.env.TELLURION_APP_ORIGIN ?? 'http://127.0.0.1:8080',
      ]),
    ),
  },
  build: {
    target: 'es2022',
  },
}));
