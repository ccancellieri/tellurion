import { describe, expect, it } from 'vitest';
import {
  addWorkspaceLayer,
  catalogChoicesFromTenantLinks,
  contextSearch,
  contextFromSearch,
  demoRasterMapHandoff,
  linkByRel,
  mergeCollectionPage,
  removeWorkspaceLayer,
  safeEndpointHref,
  selectCatalog,
  setWorkspaceLayerVisibility,
  workspaceMapIds,
  workspaceRenderPlan,
  withoutWorkspacePreview,
  type WorkspaceLayer,
} from './map';
import type { DemoSourceResponse } from './demo-source';

const roads: WorkspaceLayer = {
  id: 'roads',
  title: 'Road network',
  visible: true,
};

describe('contextFromSearch', () => {
  it('uses defaults when the URL has no workspace context', () => {
    expect(contextFromSearch('', 'public', 'default')).toEqual({
      tenantId: 'public',
      catalogId: 'default',
    });
  });

  it('reads tenant and catalog context from the URL', () => {
    expect(contextFromSearch('?tenant=city&catalog=transport', 'public', 'default')).toEqual({
      tenantId: 'city',
      catalogId: 'transport',
    });
  });

  it('does not accept blank context values', () => {
    expect(contextFromSearch('?tenant=%20&catalog=', 'public', 'default')).toEqual({
      tenantId: 'public',
      catalogId: 'default',
    });
  });

  it('serializes an encoded workspace context', () => {
    expect(contextSearch({ tenantId: 'city team', catalogId: 'roads/water' })).toBe(
      '?tenant=city+team&catalog=roads%2Fwater',
    );
  });
});

describe('tenant catalog choices', () => {
  const origin = 'https://tellurion.example';
  const tenantDocument = '/city';
  const links = [
    { href: '/city/features/catalogs/base%20maps', rel: 'features' },
    { href: '/city/tiles/catalogs/base%20maps', rel: 'tiles' },
    { href: '/city/features/catalogs/analysis', rel: 'features' },
    { href: '/other/features/catalogs/private', rel: 'features' },
    { href: 'javascript:alert(1)', rel: 'features' },
  ];

  it('keeps unique advertised Features roots in order', () => {
    expect(catalogChoicesFromTenantLinks(links, tenantDocument, origin, 'city')).toEqual([
      {
        id: 'base maps',
        featuresRoot: 'https://tellurion.example/city/features/catalogs/base%20maps',
      },
      {
        id: 'analysis',
        featuresRoot: 'https://tellurion.example/city/features/catalogs/analysis',
      },
    ]);
  });

  it('selects an advertised request and explains a fallback', () => {
    const choices = catalogChoicesFromTenantLinks(links, tenantDocument, origin, 'city');
    expect(selectCatalog(choices, 'analysis', 'city')).toEqual({ choice: choices[1] });
    expect(selectCatalog(choices, 'missing', 'city')).toEqual({
      choice: choices[0],
      reason: 'Catalog “missing” is not advertised; opened “base maps” instead.',
    });
    expect(selectCatalog([], 'missing', 'city')).toEqual({
      choice: null,
      reason: 'Tenant “city” does not advertise a Features catalog.',
    });
  });
});

describe('linkByRel', () => {
  it('matches a full OGC relation URI by its final segment', () => {
    const link = {
      href: '/tiles',
      rel: 'http://www.opengis.net/def/rel/ogc/1.0/tilesets-vector',
    };
    expect(linkByRel([link], 'tilesets-vector')).toBe(link);
  });
});

describe('workspace layer state', () => {
  it('adds a collection only once', () => {
    expect(addWorkspaceLayer([roads], roads)).toEqual([roads]);
  });

  it('adds a new collection as a visible layer', () => {
    expect(addWorkspaceLayer([], roads)).toEqual([roads]);
  });

  it('changes a layer visibility without changing the other layers', () => {
    const buildings: WorkspaceLayer = { id: 'buildings', title: 'Buildings', visible: true };
    expect(setWorkspaceLayerVisibility([roads, buildings], 'roads', false)).toEqual([
      { ...roads, visible: false },
      buildings,
    ]);
  });

  it('removes the requested layer', () => {
    expect(removeWorkspaceLayer([roads], 'roads')).toEqual([]);
  });
});

describe('ephemeral demo map handoff', () => {
  const source: DemoSourceResponse = {
    id: 'opaque-source',
    format: 'tiled-geotiff',
    transport: 'range-native',
    revision: 'strong',
    capability_state: 'ready',
    extent: [12, 39, 15, 42] as [number, number, number, number],
    geometryType: null,
    srid: null,
    numberMatched: null,
    properties: [],
    attribution: 'Verified attribution',
    limits: { expires_in_seconds: 900, max_live_sources: 3, max_concurrent_operations: 2 },
    links: {
      self_href: '/demo/sources/opaque-source',
      tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
    },
  };

  it('keeps the exact server-advertised same-origin demo tile template', () => {
    expect(demoRasterMapHandoff(source, 'https://tellurion.example')).toEqual({
      sourceId: 'demo-source-opaque-source',
      layerId: 'demo-layer-opaque-source',
      template: 'https://tellurion.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
      extent: source.extent,
      attribution: 'Verified attribution',
    });
  });

  it('does not create a map source from an unadvertised external template', () => {
    expect(
      demoRasterMapHandoff(
        { ...source, links: { ...source.links, tile_template: 'https://outside.example/tile/{z}/{y}/{x}.png' } },
        'https://tellurion.example',
      ),
    ).toBeNull();
  });

  it('does not accept an advertised tile route for another opaque source id', () => {
    expect(
      demoRasterMapHandoff(
        { ...source, links: { ...source.links, tile_template: '/demo/sources/other/tiles/WebMercatorQuad/{z}/{y}/{x}.png' } },
        'https://tellurion.example',
      ),
    ).toBeNull();
  });
});

describe('workspace preview state', () => {
  it('drops only the removed collection preview', () => {
    const previews = new Map([
      ['roads', [{ id: 1 }]],
      ['buildings', [{ id: 2 }]],
    ]);
    expect([...withoutWorkspacePreview(previews, 'roads')]).toEqual([
      ['buildings', [{ id: 2 }]],
    ]);
    expect(previews.has('roads')).toBe(true);
  });
});

describe('mergeCollectionPage', () => {
  it('appends unique collections and follows the authoritative next link', () => {
    expect(
      mergeCollectionPage(
        [{ id: 'roads' }],
        {
          collections: [{ id: 'roads' }, { id: 'water' }],
          links: [{ href: '?cursor=two', rel: 'next' }],
        },
        '/city/features/catalogs/base/collections?limit=100',
        'https://tellurion.example',
      ),
    ).toEqual({
      collections: [{ id: 'roads' }, { id: 'water' }],
      nextHref:
        'https://tellurion.example/city/features/catalogs/base/collections?cursor=two',
    });
  });
});

describe('workspaceRenderPlan', () => {
  const origin = 'https://tellurion.example';
  const collectionDocumentHref = '/city/features/catalogs/base/collections/roads';
  const tileSetDocumentHref =
    '/city/tiles/catalogs/base/collections/roads/tiles/WebMercatorQuad';
  const collectionLinks = [{ href: `${collectionDocumentHref}/items`, rel: 'items' }];
  const tileLinks = [
    {
      href: `${tileSetDocumentHref}/{tileMatrix}/{tileRow}/{tileCol}.mvt`,
      rel: 'item',
      type: 'application/vnd.mapbox-vector-tile',
      templated: true,
    },
    {
      href: `${tileSetDocumentHref}/{tileMatrix}/{tileRow}/{tileCol}.png`,
      rel: 'item',
      type: 'image/png',
      templated: true,
    },
  ];
  const limits = [
    { tileMatrix: '2' },
    { tileMatrix: '3' },
    { tileMatrix: '4' },
  ];

  it('prefers MVT and preserves every distinct real source layer', () => {
    expect(
      workspaceRenderPlan({
        collectionLinks,
        collectionDocumentHref,
        tileSetDocumentHref,
        origin,
        tileSet: {
          tileMatrixSetId: 'WebMercatorQuad',
          tileMatrixSetLimits: limits,
          mediaTypes: ['application/vnd.mapbox-vector-tile', 'image/png'],
          layers: [{ id: 'roads' }, { id: '' }, { id: 'roads' }, { id: 'bridges' }],
          links: tileLinks,
        },
      }),
    ).toEqual({
      mode: 'vector',
      template:
        'https://tellurion.example/city/tiles/catalogs/base/collections/roads/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt',
      sourceLayers: ['roads', 'bridges'],
      minzoom: 2,
      maxzoom: 4,
    });
  });

  it('uses the separately advertised PNG template when vector layers are unusable', () => {
    expect(
      workspaceRenderPlan({
        collectionLinks,
        collectionDocumentHref,
        tileSetDocumentHref,
        origin,
        tileSet: {
          tileMatrixSetId: 'WebMercatorQuad',
          tileMatrixSetLimits: limits,
          mediaTypes: ['application/vnd.mapbox-vector-tile', 'image/png'],
          layers: [],
          links: tileLinks,
        },
      }),
    ).toEqual({
      mode: 'raster',
      template:
        'https://tellurion.example/city/tiles/catalogs/base/collections/roads/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
      minzoom: 2,
      maxzoom: 4,
      reason: 'Vector tiles have no advertised source layers; using raster tiles.',
    });
  });

  it('moves from a failed vector source to the advertised raster template', () => {
    expect(
      workspaceRenderPlan({
        collectionLinks,
        collectionDocumentHref,
        tileSetDocumentHref,
        origin,
        excludedModes: ['vector'],
        fallbackReason: 'Vector tile requests failed; using the next advertised representation.',
        tileSet: {
          tileMatrixSetId: 'WebMercatorQuad',
          tileMatrixSetLimits: limits,
          mediaTypes: ['application/vnd.mapbox-vector-tile', 'image/png'],
          layers: [{ id: 'roads' }],
          links: tileLinks,
        },
      }),
    ).toEqual({
      mode: 'raster',
      template:
        'https://tellurion.example/city/tiles/catalogs/base/collections/roads/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
      minzoom: 2,
      maxzoom: 4,
      reason: 'Vector tile requests failed; using the next advertised representation.',
    });
  });

  it('moves from failed tile sources to one bounded feature page', () => {
    expect(
      workspaceRenderPlan({
        collectionLinks,
        collectionDocumentHref,
        tileSetDocumentHref,
        origin,
        excludedModes: ['vector', 'raster'],
        fallbackReason: 'Raster tile requests failed; using the next advertised representation.',
        tileSet: {
          tileMatrixSetId: 'WebMercatorQuad',
          tileMatrixSetLimits: limits,
          mediaTypes: ['application/vnd.mapbox-vector-tile', 'image/png'],
          layers: [{ id: 'roads' }],
          links: tileLinks,
        },
      }),
    ).toEqual({
      mode: 'preview',
      itemsHref:
        'https://tellurion.example/city/features/catalogs/base/collections/roads/items',
      reason: 'Raster tile requests failed; using the next advertised representation.',
    });
  });

  it('falls back to one safe items page when tile metadata cannot be used', () => {
    expect(
      workspaceRenderPlan({
        collectionLinks,
        collectionDocumentHref,
        tileSetDocumentHref,
        origin,
        fallbackReason: 'TileSet discovery failed.',
        tileSet: null,
      }),
    ).toEqual({
      mode: 'preview',
      itemsHref:
        'https://tellurion.example/city/features/catalogs/base/collections/roads/items',
      reason: 'TileSet discovery failed.',
    });
  });

  it('rejects unsupported matrix sets and unsafe items links', () => {
    expect(
      workspaceRenderPlan({
        collectionLinks: [{ href: 'javascript:alert(1)', rel: 'items' }],
        collectionDocumentHref,
        tileSetDocumentHref,
        origin,
        tileSet: {
          tileMatrixSetId: 'WorldCRS84Quad',
          tileMatrixSetLimits: limits,
          mediaTypes: ['application/vnd.mapbox-vector-tile'],
          layers: [{ id: 'roads' }],
          links: tileLinks,
        },
      }),
    ).toEqual({
      mode: 'unavailable',
      reason: 'No compatible tile or feature representation is advertised.',
    });
  });
});

describe('workspaceMapIds', () => {
  it('creates unique opaque layer ids without embedding advertised source-layer names', () => {
    const plan = {
      mode: 'vector' as const,
      template: 'https://tellurion.example/tiles/{z}/{y}/{x}.mvt',
      sourceLayers: ['roads / private', 'water:main'],
      minzoom: 0,
      maxzoom: 14,
    };

    const ids = workspaceMapIds('city/roads', plan);

    expect(ids.sourceId).toMatch(/^workspace-source-[a-z0-9-]+$/);
    expect(ids.layerIds).toHaveLength(6);
    expect(new Set(ids.layerIds).size).toBe(ids.layerIds.length);
    expect(ids.layerIds.every((id) => /^[a-z0-9-]+$/.test(id))).toBe(true);
    expect(ids.layerIds.join(' ')).not.toContain('roads / private');
    expect(ids.layerIds.join(' ')).not.toContain('water:main');
  });
});

describe('safeEndpointHref', () => {
  const origin = 'https://tellurion.example';

  it('keeps a same-origin relative endpoint navigable', () => {
    expect(safeEndpointHref('/public/features', origin)).toBe('/public/features');
  });

  it('allows an http or https endpoint', () => {
    expect(safeEndpointHref('https://catalog.example/collections', origin)).toBe(
      'https://catalog.example/collections',
    );
  });

  it('rejects an unsupported endpoint scheme', () => {
    expect(safeEndpointHref('javascript:alert(1)', origin)).toBeNull();
  });

  it('rejects a protocol-relative endpoint on another origin', () => {
    expect(safeEndpointHref('//untrusted.example/collections', origin)).toBeNull();
  });
});
