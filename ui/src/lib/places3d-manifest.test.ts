import { describe, expect, it } from 'vitest';
import {
  MIN_AFFINE_3D_ZOOM,
  MAX_VISIBLE_3D_TILES,
  buildViewportTileset,
} from './places3d-manifest';

const rootUrl = 'https://tellurion.example/public/3dtiles/catalogs/default/collections/monaco_buildings/3dtiles';
const extent: [number, number, number, number] = [
  7.40919828414917,
  43.725494384765625,
  7.439594745635986,
  43.75098419189453,
];

function implicitRoot(contentUri = `${rootUrl}/tiles/{level}/{y}/{x}.glb`): unknown {
  return {
    asset: { version: '1.1' },
    geometricError: 156543.03392804097,
    root: {
      boundingVolume: { region: [-Math.PI, -Math.PI / 2, Math.PI, Math.PI / 2, 0, 10_000] },
      content: { uri: contentUri },
      geometricError: 156543.03392804097,
      refine: 'REPLACE',
      implicitTiling: {
        subdivisionScheme: 'QUADTREE',
        subtreeLevels: 19,
        availableLevels: 19,
        subtrees: { uri: `${rootUrl}/subtrees/{level}/{y}/{x}.subtree` },
      },
    },
  };
}

describe('bounded 3D places manifest', () => {
  it('turns the live 19-level root into only the WebMercatorQuad tiles intersecting the viewport', () => {
    const result = buildViewportTileset({
      tileset: implicitRoot(),
      rootUrl,
      pageUrl: 'https://tellurion.example/ui/',
      collectionExtent: extent,
      viewport: extent,
      requestedZoom: 16,
    });

    expect(result.kind).toBe('manifest');
    if (result.kind !== 'manifest') return;
    expect(result.zoom).toBe(16);
    expect(result.tileCount).toBe(49);
    expect(result.tileset.asset).toEqual({ version: '1.1', gltfUpAxis: 'Z' });
    expect(result.tileset.root).not.toHaveProperty('implicitTiling');
    expect(result.tileset.root.children).toHaveLength(49);
    expect(result.tileset.root.children[0].content.uri).toBe(
      `${rootUrl}/tiles/16/23893/34116.glb`,
    );
    expect(result.tileset.root.children.at(-1)?.content.uri).toBe(
      `${rootUrl}/tiles/16/23899/34122.glb`,
    );
  });

  it('emits an ECEF transform for real Monaco tile-local X east, Y south, and Z-up metres', () => {
    const result = buildViewportTileset({
      tileset: implicitRoot(),
      rootUrl,
      pageUrl: 'https://tellurion.example/ui/',
      collectionExtent: extent,
      viewport: [7.419, 43.738, 7.421, 43.74],
      requestedZoom: 18,
    });

    expect(result.kind).toBe('manifest');
    if (result.kind !== 'manifest') return;
    const tile = result.tileset.root.children[0];
    const matrix = tile.transform;
    const center = [
      matrix[12] + matrix[0] / 2 + matrix[4] / 2,
      matrix[13] + matrix[1] / 2 + matrix[5] / 2,
      matrix[14] + matrix[2] / 2 + matrix[6] / 2,
    ];

    expect(Math.hypot(matrix[0], matrix[1], matrix[2])).toBeCloseTo(110.63, 1);
    expect(Math.hypot(matrix[4], matrix[5], matrix[6])).toBeCloseTo(110.24, 1);
    expect(Math.hypot(matrix[8], matrix[9], matrix[10])).toBeCloseTo(1, 12);
    expect(center[0]).toBeGreaterThan(4_500_000);
    expect(center[1]).toBeGreaterThan(500_000);
    expect(center[2]).toBeGreaterThan(4_300_000);
    expect(matrix[8]).toBeGreaterThan(0);
    expect(matrix[9]).toBeGreaterThan(0);
    expect(matrix[10]).toBeGreaterThan(0);
    expect(tile.boundingVolume.region[0]).toBeLessThan(tile.boundingVolume.region[2]);
    expect(tile.boundingVolume.region[1]).toBeLessThan(tile.boundingVolume.region[3]);
  });

  it('keeps the z14 Monaco tile and MVT-buffer corners within 0.22 metres of WGS84 ECEF', () => {
    const viewport: [number, number, number, number] = [7.4092, 43.749, 7.41, 43.7509];
    const result = buildViewportTileset({
      tileset: implicitRoot(),
      rootUrl,
      pageUrl: 'https://tellurion.example/ui/',
      collectionExtent: extent,
      viewport,
      requestedZoom: 14,
    });
    expect(result.kind).toBe('manifest');
    if (result.kind !== 'manifest') return;

    const matrix = result.tileset.root.children[0].transform;
    const exactEcef = (u: number, v: number): number[] => {
      const side = 2 ** 14;
      const longitude = ((8529 + u) / side * 2 - 1) * Math.PI;
      const latitude = Math.atan(Math.sinh(Math.PI * (1 - 2 * (5973 + v) / side)));
      const eccentricitySquared = 6.69437999014e-3;
      const radius = 6_378_137 / Math.sqrt(1 - eccentricitySquared * Math.sin(latitude) ** 2);
      return [
        radius * Math.cos(latitude) * Math.cos(longitude),
        radius * Math.cos(latitude) * Math.sin(longitude),
        radius * (1 - eccentricitySquared) * Math.sin(latitude),
      ];
    };
    const errors = [-0.0625, 0, 1, 1.0625].flatMap((u) =>
      [-0.0625, 0, 1, 1.0625].map((v) => {
        const approximate = [
          matrix[12] + matrix[0] * u + matrix[4] * v,
          matrix[13] + matrix[1] * u + matrix[5] * v,
          matrix[14] + matrix[2] * u + matrix[6] * v,
        ];
        return Math.hypot(...approximate.map((value, index) => value - exactEcef(u, v)[index]));
      }),
    );
    expect(Math.max(...errors)).toBeLessThan(0.22);
  });

  it('refuses a requested zoom below the bounded affine placement range instead of clamping', () => {
    const result = buildViewportTileset({
      tileset: implicitRoot(),
      rootUrl,
      pageUrl: 'https://tellurion.example/ui/',
      collectionExtent: extent,
      viewport: extent,
      requestedZoom: 13,
    });

    expect(result).toEqual({
      kind: 'too-coarse',
      requestedZoom: 13,
      minimumZoom: MIN_AFFINE_3D_ZOOM,
      maxTiles: MAX_VISIBLE_3D_TILES,
      tileCount: 0,
    });
  });

  it('refuses a viewport above the finite tile budget without truncating it', () => {
    const result = buildViewportTileset({
      tileset: implicitRoot(),
      rootUrl,
      pageUrl: 'https://tellurion.example/ui/',
      collectionExtent: extent,
      viewport: extent,
      requestedZoom: 18,
    });

    expect(result).toEqual({
      kind: 'too-many',
      zoom: 18,
      tileCount: 598,
      maxTiles: MAX_VISIBLE_3D_TILES,
    });
  });

  it.each([
    ['another origin', 'https://outside.example/tiles/{level}/{y}/{x}.glb'],
    ['a template outside the root', 'https://tellurion.example/tiles/{level}/{y}/{x}.glb'],
    ['reordered WebMercator coordinates', `${rootUrl}/tiles/{level}/{x}/{y}.glb`],
    ['a non-GLB suffix', `${rootUrl}/tiles/{level}/{y}/{x}.json`],
  ])('rejects %s', (_label, contentUri) => {
    expect(() => buildViewportTileset({
      tileset: implicitRoot(contentUri),
      rootUrl,
      pageUrl: 'https://tellurion.example/ui/',
      collectionExtent: extent,
      viewport: extent,
      requestedZoom: 16,
    })).toThrow(/3D tileset/);
  });

  it('preserves an encoded collection id while normalizing only template token escapes', () => {
    const encodedRootUrl = rootUrl.replace('monaco_buildings', 'monaco%20buildings');
    const result = buildViewportTileset({
      tileset: implicitRoot(`${encodedRootUrl}/tiles/{level}/{y}/{x}.glb`),
      rootUrl: encodedRootUrl,
      pageUrl: 'https://tellurion.example/ui/',
      collectionExtent: extent,
      viewport: extent,
      requestedZoom: 16,
    });

    expect(result.kind).toBe('manifest');
    if (result.kind !== 'manifest') return;
    expect(result.tileset.root.children[0].content.uri).toBe(
      `${encodedRootUrl}/tiles/16/23893/34116.glb`,
    );
  });

  it.each([
    ['missing root', { asset: { version: '1.1' } }],
    ['wrong scheme', { ...implicitRoot() as object, asset: { version: '1.1' }, root: { implicitTiling: { subdivisionScheme: 'OCTREE' } } }],
    ['invalid levels', { ...implicitRoot() as object, root: { ...(implicitRoot() as { root: object }).root, implicitTiling: { subdivisionScheme: 'QUADTREE', availableLevels: 0 } } }],
  ])('rejects malformed input: %s', (_label, tileset) => {
    expect(() => buildViewportTileset({
      tileset,
      rootUrl,
      pageUrl: 'https://tellurion.example/ui/',
      collectionExtent: extent,
      viewport: extent,
      requestedZoom: 16,
    })).toThrow(/3D tileset/);
  });
});
