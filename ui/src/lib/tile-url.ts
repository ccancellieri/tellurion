// Adapter between MapLibre GL JS's tile URL convention and Tellurion's OGC
// API Tiles route, which is scoped under a tenant/catalog prefix and orders
// path segments row-before-column:
//
//   /{tenant}/tiles/catalogs/{catalog}/collections/{cid}/tiles/WebMercatorQuad/{tileMatrix}/{tileRow}/{tileCol}
//
// MapLibre replaces the literal substrings "{z}", "{x}" and "{y}" wherever
// they appear in a source's `tiles` template, so placing them in
// `{z}/{y}/{x}` order in the template string is enough to satisfy the
// server's row-before-column contract — no runtime coordinate juggling is
// needed. This module exists so that mapping is written once, in one place,
// and is verifiable without a browser or a running server (see
// `tile-url.test.ts`), rather than being an implicit assumption baked
// separately into every panel that adds a tile source.

export type TileFormat = 'mvt' | 'png';

export interface AdvertisedTileLink {
  href: string;
  rel: string;
  type?: string;
  templated?: boolean;
}

/** Resolves a server-advertised data link against the document that carried
 * it. Programmatic reads stay on the UI's current origin; display-only links
 * use the broader navigation policy in map.ts. */
export function resolveDataHref(
  href: string,
  documentHref: string,
  origin: string,
): string | null {
  try {
    const expectedOrigin = new URL(origin).origin;
    const documentUrl = new URL(documentHref, expectedOrigin);
    const resolved = new URL(href, documentUrl);
    if (
      documentUrl.origin !== expectedOrigin ||
      resolved.origin !== expectedOrigin ||
      (resolved.protocol !== 'http:' && resolved.protocol !== 'https:')
    ) {
      return null;
    }
    return resolved.href;
  } catch {
    return null;
  }
}

/** Converts the three OGC API Tiles template variables into MapLibre's
 * tokens without letting URL canonicalization percent-encode the braces.
 * Unsupported or incomplete URI templates are rejected instead of guessed. */
export function maplibreTileTemplate(
  link: AdvertisedTileLink,
  documentHref: string,
  origin: string,
): string | null {
  if (!link.templated) return null;

  const replacements = [
    ['{tileMatrix}', '__TELLURION_TILE_MATRIX__', '{z}'],
    ['{tileRow}', '__TELLURION_TILE_ROW__', '{y}'],
    ['{tileCol}', '__TELLURION_TILE_COL__', '{x}'],
  ] as const;
  let protectedHref = link.href;
  for (const [serverToken, placeholder] of replacements) {
    if (protectedHref.split(serverToken).length !== 2) return null;
    protectedHref = protectedHref.replace(serverToken, placeholder);
  }
  if (/[{}]/.test(protectedHref)) return null;

  const resolved = resolveDataHref(protectedHref, documentHref, origin);
  if (!resolved) return null;
  return replacements.reduce(
    (template, [, placeholder, maplibreToken]) =>
      template.replace(placeholder, maplibreToken),
    resolved,
  );
}

/** Escapes a path segment for embedding in a URL template. */
function encodeSegment(value: string): string {
  return encodeURIComponent(value);
}

/**
 * Builds a MapLibre tile source URL template for the unstyled tile lane
 * (`/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/tiles/WebMercatorQuad/...`).
 * `format` selects the `.mvt` (vector) or `.png` (raster) suffix, which the
 * server's format negotiation treats as authoritative over the `Accept`
 * header.
 */
export function buildTileUrlTemplate(
  apiBase: string,
  tenantId: string,
  catalogId: string,
  collectionId: string,
  format: TileFormat,
): string {
  const suffix = format === 'mvt' ? 'mvt' : 'png';
  return (
    `${apiBase}/${encodeSegment(tenantId)}/tiles/catalogs/${encodeSegment(catalogId)}` +
    `/collections/${encodeSegment(collectionId)}` +
    `/tiles/WebMercatorQuad/{z}/{y}/{x}.${suffix}`
  );
}

/**
 * Builds a MapLibre raster source URL template for the styled tile lane
 * (`/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/styles/{styleId}/map/tiles/WebMercatorQuad/...`).
 * This lane is raster-only (see the server's `styled_tile` handler), so
 * there is no format parameter. It's still served under the tiles protocol
 * prefix, not the styles one — the tiles crate owns this route.
 */
export function buildStyledTileUrlTemplate(
  apiBase: string,
  tenantId: string,
  catalogId: string,
  collectionId: string,
  styleId: string,
): string {
  return (
    `${apiBase}/${encodeSegment(tenantId)}/tiles/catalogs/${encodeSegment(catalogId)}` +
    `/collections/${encodeSegment(collectionId)}` +
    `/styles/${encodeSegment(styleId)}/map/tiles/WebMercatorQuad/{z}/{y}/{x}.png`
  );
}

/**
 * Simulates MapLibre's own token substitution (a plain string replace of
 * the literal `{z}`/`{x}`/`{y}` tokens) so a template built by this module
 * can be proven, without MapLibre or a network call, to place the row
 * (`y`) before the column (`x`) in the resulting path — the server's
 * `{tileMatrix}/{tileRow}/{tileCol}` contract.
 */
export function substituteTileTemplate(
  template: string,
  z: number,
  x: number,
  y: number,
): string {
  return template
    .replace('{z}', String(z))
    .replace('{x}', String(x))
    .replace('{y}', String(y));
}
