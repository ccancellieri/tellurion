import { describe, expect, it } from 'vitest';
import {
  buildStyledTileUrlTemplate,
  buildTileUrlTemplate,
  maplibreTileTemplate,
  resolveDataHref,
  substituteTileTemplate,
} from './tile-url';

describe('buildTileUrlTemplate', () => {
  it('places the MapLibre z/x/y tokens in the template', () => {
    const template = buildTileUrlTemplate('', 'public', 'default', 'demo', 'mvt');
    expect(template).toBe(
      '/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt',
    );
  });

  it('uses the .png suffix for the raster format', () => {
    const template = buildTileUrlTemplate('', 'public', 'default', 'demo', 'png');
    expect(template.endsWith('.png')).toBe(true);
  });

  it('percent-encodes the collection id', () => {
    const template = buildTileUrlTemplate('', 'public', 'default', 'my collection', 'mvt');
    expect(template).toContain('/collections/my%20collection/');
  });

  it('percent-encodes the tenant and catalog ids', () => {
    const template = buildTileUrlTemplate('', 'my tenant', 'my catalog', 'demo', 'mvt');
    expect(template).toBe(
      '/my%20tenant/tiles/catalogs/my%20catalog/collections/demo/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt',
    );
  });

  it('resolves against a non-empty API base', () => {
    const template = buildTileUrlTemplate(
      'http://localhost:8080',
      'public',
      'default',
      'demo',
      'mvt',
    );
    expect(
      template.startsWith(
        'http://localhost:8080/public/tiles/catalogs/default/collections/demo/',
      ),
    ).toBe(true);
  });
});

describe('buildStyledTileUrlTemplate', () => {
  it('builds the styled raster lane path with no format token', () => {
    const template = buildStyledTileUrlTemplate('', 'public', 'default', 'demo', 'default');
    expect(template).toBe(
      '/public/tiles/catalogs/default/collections/demo/styles/default/map/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
    );
  });
});

describe('substituteTileTemplate places the row before the column', () => {
  it('matches the server route order: {tileMatrix}/{tileRow}/{tileCol}', () => {
    const template = buildTileUrlTemplate('', 'public', 'default', 'demo', 'mvt');
    // MapLibre would request tile z=5, x=3 (column), y=7 (row).
    const resolved = substituteTileTemplate(template, 5, 3, 7);
    expect(resolved).toBe(
      '/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/5/7/3.mvt',
    );
  });

  it('never swaps row and column even when they differ', () => {
    const template = buildTileUrlTemplate('', 'public', 'default', 'demo', 'png');
    const resolved = substituteTileTemplate(template, 2, 1, 3);
    const segments = resolved.split('/');
    const tileMatrix = segments[segments.length - 3];
    const tileRow = segments[segments.length - 2];
    const tileCol = segments[segments.length - 1].replace('.png', '');
    expect(tileMatrix).toBe('2');
    expect(tileRow).toBe('3'); // y
    expect(tileCol).toBe('1'); // x
  });

  it('substitutes the styled-lane template the same way', () => {
    const template = buildStyledTileUrlTemplate('', 'public', 'default', 'demo', 'default');
    const resolved = substituteTileTemplate(template, 4, 9, 2);
    expect(resolved).toBe(
      '/public/tiles/catalogs/default/collections/demo/styles/default/map/tiles/WebMercatorQuad/4/2/9.png',
    );
  });
});

describe('resolveDataHref', () => {
  const origin = 'https://tellurion.example';
  const documentHref = '/public/features/catalogs/default/collections';

  it('resolves a relative advertised link against its source document', () => {
    expect(resolveDataHref('?cursor=next', documentHref, origin)).toBe(
      'https://tellurion.example/public/features/catalogs/default/collections?cursor=next',
    );
  });

  it('rejects a cross-origin data link', () => {
    expect(resolveDataHref('https://other.example/collections', documentHref, origin)).toBeNull();
  });

  it('rejects a non-http data link', () => {
    expect(resolveDataHref('javascript:alert(1)', documentHref, origin)).toBeNull();
  });
});

describe('maplibreTileTemplate', () => {
  const origin = 'https://tellurion.example';
  const documentHref =
    '/public/tiles/catalogs/default/collections/roads/tiles/WebMercatorQuad';

  it('converts the advertised row-first MVT template into MapLibre tokens', () => {
    expect(
      maplibreTileTemplate(
        {
          href: `${documentHref}/{tileMatrix}/{tileRow}/{tileCol}.mvt`,
          rel: 'item',
          type: 'application/vnd.mapbox-vector-tile',
          templated: true,
        },
        documentHref,
        origin,
      ),
    ).toBe(
      'https://tellurion.example/public/tiles/catalogs/default/collections/roads/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt',
    );
  });

  it('preserves a PNG template query string', () => {
    expect(
      maplibreTileTemplate(
        {
          href: `${documentHref}/{tileMatrix}/{tileRow}/{tileCol}?f=png&token=public`,
          rel: 'item',
          type: 'image/png',
          templated: true,
        },
        documentHref,
        origin,
      ),
    ).toBe(
      'https://tellurion.example/public/tiles/catalogs/default/collections/roads/tiles/WebMercatorQuad/{z}/{y}/{x}?f=png&token=public',
    );
  });

  it('preserves supported tokens in an absolute same-origin template', () => {
    expect(
      maplibreTileTemplate(
        {
          href: 'https://tellurion.example/tiles/{tileMatrix}/{tileRow}/{tileCol}.png',
          rel: 'item',
          type: 'image/png',
          templated: true,
        },
        documentHref,
        origin,
      ),
    ).toBe('https://tellurion.example/tiles/{z}/{y}/{x}.png');
  });

  it('rejects an unmarked or incomplete template', () => {
    expect(
      maplibreTileTemplate(
        {
          href: `${documentHref}/{tileMatrix}/{tileRow}/0.mvt`,
          rel: 'item',
          type: 'application/vnd.mapbox-vector-tile',
        },
        documentHref,
        origin,
      ),
    ).toBeNull();
  });

  it('rejects unsupported URI-template variables and cross-origin templates', () => {
    expect(
      maplibreTileTemplate(
        {
          href: `${documentHref}/{tileMatrix}/{tileRow}/{tileCol}{?f}`,
          rel: 'item',
          type: 'image/png',
          templated: true,
        },
        documentHref,
        origin,
      ),
    ).toBeNull();
    expect(
      maplibreTileTemplate(
        {
          href: 'https://other.example/{tileMatrix}/{tileRow}/{tileCol}.mvt',
          rel: 'item',
          type: 'application/vnd.mapbox-vector-tile',
          templated: true,
        },
        documentHref,
        origin,
      ),
    ).toBeNull();
  });
});
