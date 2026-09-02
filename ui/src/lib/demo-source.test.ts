import { describe, expect, it } from 'vitest';
import {
  demoTileTemplateFromAdvertisedLink,
  validateDemoSourceResponse,
  eligibleDemoSourceUrl,
  resetDemoWorkflow,
  startDemoInspection,
  publishDemoMap,
  type DemoSourceResponse,
} from './demo-source';

const response: DemoSourceResponse = {
  id: 'opaque-source',
  format: 'tiled-geotiff',
  transport: 'range-native',
  revision: 'strong',
  capability_state: 'ready',
  extent: [12, 39, 15, 42],
  geometryType: null,
  srid: null,
  numberMatched: null,
  properties: [],
  attribution: 'Verified source attribution',
  limits: { expires_in_seconds: 900, max_live_sources: 3, max_concurrent_operations: 2 },
  links: {
    self_href: '/demo/sources/opaque-source',
    tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
  },
};

const wireResponse = {
  id: response.id,
  format: response.format,
  transport: response.transport,
  revision: response.revision,
  capability_state: response.capability_state,
  extent: response.extent,
  geometry_type: null,
  srid: null,
  number_matched: null,
  properties: [],
  attribution: response.attribution,
  limits: response.limits,
  links: response.links,
};

describe('public demo URL eligibility', () => {
  it.each([
    'http://example.org/source.tif',
    'https://example.org:444/source.tif',
    'https://user@example.org/source.tif',
    'https://example.org/source.tif?token=secret',
    'https://example.org/source.tif#fragment',
    `https://example.org/${'x'.repeat(2050)}`,
  ])('rejects unsafe browser input without echoing the locator', (url) => {
    const result = eligibleDemoSourceUrl(url);
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.message).not.toContain(url);
  });

  it('accepts an ordinary HTTPS URL on its effective default port', () => {
    expect(eligibleDemoSourceUrl('https://data.example.org/worldcover.tif')).toEqual({
      ok: true,
      value: 'https://data.example.org/worldcover.tif',
    });
  });
});

describe('demo workflow state', () => {
  it('erases the raw locator as soon as inspection succeeds', () => {
    const inspecting = startDemoInspection('https://data.example.org/worldcover.tif');
    const inspected = inspecting.succeed(response);

    expect(inspected.phase).toBe('inspect');
    expect(inspected).not.toHaveProperty('rawUrl');
    expect(JSON.stringify(inspected)).not.toContain('data.example.org');
  });

  it('keeps an opaque failure message and lets the visitor reset', () => {
    const failed = startDemoInspection('https://data.example.org/private.tif').fail(422);
    expect(failed.message).not.toContain('data.example.org');
    expect(resetDemoWorkflow(failed)).toEqual({ phase: 'choose', rawUrl: '' });
  });

  it('requires inspection before the local map handoff', () => {
    expect(() => publishDemoMap({ phase: 'choose', rawUrl: '' })).toThrow(
      'Inspect a source before opening its map.',
    );
    const map = publishDemoMap(startDemoInspection('https://data.example.org/x.tif').succeed(response));
    expect(map.phase).toBe('map');
    expect(map.source.id).toBe('opaque-source');
  });
});

describe('server-advertised tile handoff', () => {
  it('accepts only a same-origin demo tile template supplied by the server', () => {
    expect(
      demoTileTemplateFromAdvertisedLink(response.links.tile_template, 'https://tellurion.example'),
    ).toBe('https://tellurion.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png');
  });

  it('rejects external and altered non-demo templates instead of rebuilding them', () => {
    expect(
      demoTileTemplateFromAdvertisedLink(
        'https://elsewhere.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
        'https://tellurion.example',
      ),
    ).toBeNull();
    expect(
      demoTileTemplateFromAdvertisedLink(
        '/public/tiles/catalogs/default/collections/source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
        'https://tellurion.example',
      ),
    ).toBeNull();
  });
});

describe('server response binding', () => {
  it('binds both advertised routes to the exact opaque response id', () => {
    expect(validateDemoSourceResponse(wireResponse, 'https://tellurion.example')).toMatchObject({
      id: 'opaque-source',
      links: {
        self_href: 'https://tellurion.example/demo/sources/opaque-source',
        tile_template: 'https://tellurion.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
      },
    });
  });

  it('rejects an id whose advertised self or tile route names another source', () => {
    expect(
      validateDemoSourceResponse(
        { ...wireResponse, links: { ...wireResponse.links, self_href: '/demo/sources/other-source' } },
        'https://tellurion.example',
      ),
    ).toBeNull();
    expect(
      validateDemoSourceResponse(
        { ...wireResponse, links: { ...wireResponse.links, tile_template: '/demo/sources/other-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png' } },
        'https://tellurion.example',
      ),
    ).toBeNull();
  });

  it.each([
    { ...wireResponse, format: 'geoparquet' },
    { ...wireResponse, transport: 'bounded-spool' },
    { ...wireResponse, revision: 'weak' },
    { ...wireResponse, capability_state: 'queued' },
    { ...wireResponse, limits: { ...wireResponse.limits, expires_in_seconds: 0 } },
    { ...wireResponse, id: 'x'.repeat(129), links: { self_href: `/demo/sources/${'x'.repeat(129)}`, tile_template: `/demo/sources/${'x'.repeat(129)}/tiles/WebMercatorQuad/{z}/{y}/{x}.png` } },
    { ...wireResponse, extent: [12, 42, 15, 39] },
  ])('fails closed for an invalid public-demo response', (invalid) => {
    expect(validateDemoSourceResponse(invalid, 'https://tellurion.example')).toBeNull();
  });

  it('rejects userinfo even when the advertised route otherwise matches', () => {
    expect(
      validateDemoSourceResponse(
        { ...wireResponse, links: { ...wireResponse.links, self_href: 'https://user@tellurion.example/demo/sources/opaque-source' } },
        'https://tellurion.example',
      ),
    ).toBeNull();
    expect(
      validateDemoSourceResponse(
        { ...wireResponse, links: { ...wireResponse.links, tile_template: 'https://user@tellurion.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png' } },
        'https://tellurion.example',
      ),
    ).toBeNull();
  });

  it('admits a same-origin vector response with inspectable metadata and an advertised MVT route', () => {
    const vector = {
      ...wireResponse,
      format: 'geoparquet',
      geometry_type: 'Polygon',
      srid: 4326,
      number_matched: 7,
      properties: ['boundary_id', 'confidence'],
      links: {
        self_href: '/demo/sources/opaque-source',
        items_href: '/demo/sources/opaque-source/items',
        item_template: '/demo/sources/opaque-source/items/{featureId}',
        mvt_tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt',
        tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
      },
    };
    expect(validateDemoSourceResponse(vector, 'https://tellurion.example')).toMatchObject({
      format: 'geoparquet',
      geometryType: 'Polygon',
      srid: 4326,
      properties: ['boundary_id', 'confidence'],
      links: {
        mvt_tile_template: 'https://tellurion.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt',
      },
    });
  });

  it('refuses a vector response without its metadata or same-origin MVT capability', () => {
    const vector = {
      ...wireResponse,
      format: 'shapefile-zip',
      transport: 'bounded-zip-spool',
      geometry_type: 'LineString',
      srid: 4326,
      number_matched: 134,
      properties: ['scalerank'],
      links: {
        self_href: '/demo/sources/opaque-source',
        items_href: '/demo/sources/opaque-source/items',
        item_template: '/demo/sources/opaque-source/items/{featureId}',
        mvt_tile_template: 'https://outside.example/{z}/{y}/{x}.mvt',
        tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
      },
    };
    expect(validateDemoSourceResponse(vector, 'https://tellurion.example')).toBeNull();
  });
});
