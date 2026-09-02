import { describe, expect, it } from 'vitest';
import {
  demoFixtures,
  demoFixturesFromYaml,
  isScientificFixture,
  isExecutableFixture,
  safeDemoFixturesFromYaml,
  vectorControlsForFixture,
} from './demo-fixtures';

const inventory = `
version: 1
examples:
  - id: esa-worldcover-2021-italy
    status: active
    executable: true
    title: COG
    provider: ESA
    license: { verification: confirmed, label: CC BY 4.0, terms_url: https://creativecommons.org/licenses/by/4.0/ }
    attribution: ESA
    source_page: https://example.org/page
    url: https://example.org/a.tif
    transport: range-native
    format: tiled-geotiff
    connector: { state: ready, reason: Ready }
    activation: { state: approved, reason: Verified }
    content: { kind: object, revision: verified, expected_length: 1, expected_strong_etag: '"tag"' }
    resource: { selected: band 1, crs: EPSG:4326, extent: [0, 0, 1, 1] }
    render: { profile: categorical-land-cover, band: 1 }
  - id: grib
    status: candidate
    executable: false
    title: GRIB
    provider: GDAL
    license: { verification: review-required, label: Review required }
    attribution: Fixture
    source_page: https://example.org/grib
    url: https://example.org/a.grib2
    transport: bounded-spool
    format: grib2
    connector: { state: planned, reason: Worker pending }
    activation: { state: review-blocked, reason: Terms pending }
    content: { kind: object, revision: pinned }
`;

describe('public demo fixtures', () => {
  it('requires the supported public inventory schema version', () => {
    expect(() => demoFixturesFromYaml(inventory.replace('version: 1\n', ''))).toThrow(
      'Invalid inventory version',
    );
    expect(() => demoFixturesFromYaml(inventory.replace('version: 1', 'version: 2'))).toThrow(
      'Invalid inventory version',
    );
  });

  it('requires at least one fully valid approved executable fixture', () => {
    const parsed = demoFixturesFromYaml(inventory);
    expect(parsed.filter(isExecutableFixture).map((fixture) => fixture.id)).toEqual([
      'esa-worldcover-2021-italy',
    ]);
    const none = inventory.replace('executable: true', 'executable: false');
    expect(safeDemoFixturesFromYaml(none)).toEqual({
      fixtures: [],
      error: 'The verified example inventory is unavailable.',
    });
  });

  it('lists every promised format and enables every fully verified connector', () => {
    expect(new Set(demoFixtures.map((fixture) => fixture.format))).toEqual(
      new Set([
        'tiled-geotiff', 'geoparquet', 'shapefile-zip', 'geojson', 'geozarr',
        'grib2', 'hdf5', 'netcdf', 'geopackage', 'flatgeobuf', 'pmtiles',
      ]),
    );
    expect(demoFixtures.filter(isExecutableFixture).map((fixture) => fixture.id)).toEqual([
      'esa-worldcover-2021-italy',
      'google-microsoft-open-buildings-monaco',
      'natural-earth-110m-coastline-shapefile',
    ]);
    expect(
      demoFixtures
        .filter((fixture) => fixture.license.verification === 'review-required')
        .every((fixture) => fixture.license.termsUrl === undefined),
    ).toBe(true);
  });

  it('keeps license and connector truth from the YAML inventory', () => {
    const [cog, grib] = demoFixturesFromYaml(inventory);
    expect(cog.license.termsUrl).toBe('https://creativecommons.org/licenses/by/4.0/');
    expect(grib.license.termsUrl).toBeUndefined();
    expect(grib.executable).toBe(false);
    expect(grib.connector.state).toBe('planned');
  });

  it('requires an explicit two-dimensional slice for scientific candidates', () => {
    const [cog, grib] = demoFixturesFromYaml(inventory);
    expect(isScientificFixture(cog)).toBe(false);
    expect(isScientificFixture(grib)).toBe(true);
  });

  it('only exposes vector controls for vector formats', () => {
    const [cog] = demoFixturesFromYaml(inventory);
    expect(vectorControlsForFixture(cog)).toBe(false);
    expect(vectorControlsForFixture({ ...cog, format: 'geoparquet' })).toBe(true);
  });

  it.each([
    ['status', 'status: active', 'status: unknown'],
    ['transport', 'transport: range-native', 'transport: imaginary'],
    ['format', 'format: tiled-geotiff', 'format: imaginary'],
    ['connector', 'connector: { state: ready, reason: Ready }', 'connector: { state: connected, reason: Ready }'],
  ])('rejects a malformed %s enum without enabling the fixture', (_, expected, replacement) => {
    const invalid = inventory.replace(expected, replacement);
    expect(() => demoFixturesFromYaml(invalid)).toThrow('Invalid');
  });

  it('turns an invalid inventory into a controlled unavailable gallery', () => {
    expect(safeDemoFixturesFromYaml('examples: [not-a-fixture]')).toEqual({
      fixtures: [],
      error: 'The verified example inventory is unavailable.',
    });
  });

  it.each([
    ['source page', 'source_page: https://example.org/page', 'source_page: javascript:alert(1)'],
    ['terms URL', 'terms_url: https://creativecommons.org/licenses/by/4.0/', 'terms_url: javascript:alert(1)'],
  ])('rejects an unsafe %s before it can become a gallery link', (_, expected, replacement) => {
    expect(() => demoFixturesFromYaml(inventory.replace(expected, replacement))).toThrow('Invalid');
  });

  it.each([
    'status: candidate',
    'connector: { state: planned, reason: Waiting }',
    'activation: { state: review-blocked, reason: Waiting }',
    'license: { verification: review-required, label: Review required }',
  ])('rejects an executable fixture without every approval condition', (replacement) => {
    const invalid = inventory.replace('status: active', replacement);
    expect(() => demoFixturesFromYaml(invalid)).toThrow();
  });

  it.each([
    (fixture: (typeof demoFixtures)[number]) => ({ ...fixture, status: 'candidate' as const }),
    (fixture: (typeof demoFixtures)[number]) => ({ ...fixture, connector: { ...fixture.connector, state: 'planned' as const } }),
    (fixture: (typeof demoFixtures)[number]) => ({ ...fixture, activation: { ...fixture.activation, state: 'review-blocked' } }),
    (fixture: (typeof demoFixtures)[number]) => ({ ...fixture, license: { ...fixture.license, verification: 'review-required' as const, termsUrl: undefined } }),
  ])('does not make a candidate or unapproved fixture executable', (change) => {
    const active = demoFixtures[0];
    expect(isExecutableFixture(change(active))).toBe(false);
  });

  it.each([
    ['strong identity', 'content: { kind: object, revision: verified, expected_length: 1, expected_strong_etag: \'"tag"\' }', 'content: { kind: object, revision: verified, expected_length: 1 }'],
    ['positive length', 'expected_length: 1', 'expected_length: 0'],
    ['resource selection', '    resource: { selected: band 1, crs: EPSG:4326, extent: [0, 0, 1, 1] }\n', ''],
    ['categorical render', '    render: { profile: categorical-land-cover, band: 1 }\n', ''],
    ['eligible HTTPS URL', 'url: https://example.org/a.tif', 'url: http://example.org/a.tif'],
  ])('fails closed when the approved example loses %s', (_, expected, replacement) => {
    expect(() => demoFixturesFromYaml(inventory.replace(expected, replacement))).toThrow();
  });

  it('permits independent complete executable entries without relying on their ids', () => {
    const secondStart = inventory.indexOf('  - id: grib');
    const active = inventory.slice(inventory.indexOf('  - id:'), secondStart)
      .replace('esa-worldcover-2021-italy', 'drifted-active-entry');
    const withSecond = `${inventory.slice(0, secondStart)}${active}${inventory.slice(secondStart)}`;
    expect(demoFixturesFromYaml(withSecond).filter(isExecutableFixture).map((fixture) => fixture.id)).toEqual([
      'esa-worldcover-2021-italy',
      'drifted-active-entry',
    ]);
  });

  it('uses complete capability and identity facts rather than a fixture id to admit an executable entry', () => {
    const renamed = inventory.replace('esa-worldcover-2021-italy', 'another-verified-cog');
    expect(demoFixturesFromYaml(renamed).filter(isExecutableFixture).map((fixture) => fixture.id)).toEqual([
      'another-verified-cog',
    ]);
  });

  it('admits a ready vector object only with a complete source identity and inspectable resource facts', () => {
    const vector = `
version: 1
examples:
  - id: verified-vector
    status: active
    executable: true
    title: Verified GeoParquet
    provider: Publisher
    license: { verification: confirmed, label: ODbL 1.0, terms_url: https://example.org/terms }
    attribution: Publisher attribution
    source_page: https://example.org/source
    url: https://example.org/example.parquet
    transport: range-native
    format: geoparquet
    connector: { state: ready, reason: Ready }
    activation: { state: approved, reason: Identity verified }
    content: { kind: object, revision: verified, expected_length: 42, expected_strong_etag: '"tag"' }
    resource:
      selected: buildings
      crs: EPSG:4326
      extent: [1, 42, 2, 43]
      geometry_type: Polygon
      feature_count: 7
      attributes: [confidence]
      tested_initial_view: Fit the verified extent.
    render: { profile: vector-default }
`;
    expect(demoFixturesFromYaml(vector).filter(isExecutableFixture).map((fixture) => fixture.id)).toEqual([
      'verified-vector',
    ]);
    expect(() => demoFixturesFromYaml(vector.replace("expected_strong_etag: '\"tag\"'", ''))).toThrow();
    expect(() => demoFixturesFromYaml(vector.replace('render: { profile: vector-default }', 'render: { profile: categorical-land-cover }'))).toThrow();
    expect(() => demoFixturesFromYaml(vector.replace('      tested_initial_view: Fit the verified extent.\n', ''))).toThrow();
  });
});
