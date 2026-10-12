// @vitest-environment happy-dom

import { parse } from '@loaders.gl/core';
import { Tiles3DLoader } from '@loaders.gl/3d-tiles';
import { parsePBRMaterial } from '@luma.gl/gltf';
import { NavigationControl } from 'maplibre-gl';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const dependencies = vi.hoisted(() => {
  const layerProps: Record<string, unknown>[] = [];
  const overlaySetProps = vi.fn();
  const handlers = new Map<string, () => void>();
  const bounds = {
    getWest: vi.fn(() => 7.40919828414917),
    getSouth: vi.fn(() => 43.725494384765625),
    getEast: vi.fn(() => 7.439594745635986),
    getNorth: vi.fn(() => 43.75098419189453),
  };
  const map = {
    setPitch: vi.fn(),
    addControl: vi.fn(),
    on: vi.fn((event: string, handler: () => void) => handlers.set(event, handler)),
    off: vi.fn((event: string) => handlers.delete(event)),
    loaded: vi.fn(() => true),
    getBounds: vi.fn(() => bounds),
    getZoom: vi.fn(() => 16),
    remove: vi.fn(),
  };
  return {
    bounds,
    createdBlobs: [] as Blob[],
    handlers,
    layerProps,
    map,
    overlaySetProps,
    revokedUrls: [] as string[],
    fitToExtent: vi.fn(),
    fetchDefaultCollections: vi.fn(async () => ({
      collections: [{
        id: 'places',
        extent: {
          spatial: {
            bbox: [[7.40919828414917, 43.725494384765625, 7.439594745635986, 43.75098419189453]],
            crs: 'http://www.opengis.net/def/crs/OGC/1.3/CRS84',
          },
        },
      }],
    })),
  };
});

vi.mock('@deck.gl/maplibre', () => ({
  MapLibreOverlay: class MapLibreOverlay {
    setProps = dependencies.overlaySetProps;
  },
}));
vi.mock('@deck.gl/geo-layers', () => ({
  Tile3DLayer: class Tile3DLayer {
    constructor(props: Record<string, unknown>) {
      dependencies.layerProps.push(props);
    }
  },
}));
vi.mock('../lib/map', () => ({
  createMap: () => dependencies.map,
  fitToExtent: dependencies.fitToExtent,
}));
vi.mock('../lib/api', () => ({
  DEFAULT_CATALOG_ID: 'catalog',
  DEFAULT_TENANT_ID: 'tenant',
  fetchDefaultCollections: dependencies.fetchDefaultCollections,
}));

import './places3d-panel';

const tilesetPath = '/tenant/3dtiles/catalogs/catalog/collections/places/3dtiles';

function implicitTileset(): unknown {
  return {
    asset: { version: '1.1' },
    geometricError: 156543.03392804097,
    root: {
      boundingVolume: { region: [-Math.PI, -Math.PI / 2, Math.PI, Math.PI / 2, 0, 10_000] },
      content: { uri: `${window.location.origin}${tilesetPath}/tiles/{level}/{y}/{x}.glb` },
      geometricError: 156543.03392804097,
      refine: 'REPLACE',
      implicitTiling: {
        subdivisionScheme: 'QUADTREE',
        subtreeLevels: 19,
        availableLevels: 19,
        subtrees: { uri: `${window.location.origin}${tilesetPath}/subtrees/{level}/{y}/{x}.subtree` },
      },
    },
  };
}

function jsonResponse(value: unknown): Response {
  return new Response(JSON.stringify(value), {
    headers: { 'Content-Type': 'application/json' },
  });
}

function response(body: BodyInit, url: string): Response {
  const result = new Response(body);
  Object.defineProperty(result, 'url', { value: url });
  return result;
}

function glb(document: Record<string, unknown>): ArrayBuffer {
  const encoder = new TextEncoder();
  let source = JSON.stringify(document);
  source = source.padEnd(Math.ceil(source.length / 4) * 4, ' ');
  const json = encoder.encode(source);
  const buffer = new ArrayBuffer(20 + json.length);
  const view = new DataView(buffer);
  view.setUint32(0, 0x46546c67, true);
  view.setUint32(4, 2, true);
  view.setUint32(8, buffer.byteLength, true);
  view.setUint32(12, json.length, true);
  view.setUint32(16, 0x4e4f534a, true);
  new Uint8Array(buffer, 20).set(json);
  return buffer;
}

function minimalGlb(): ArrayBuffer {
  return glb({ asset: { version: '2.0' }, scene: 0, scenes: [{}] });
}

function texturedGlb(): ArrayBuffer {
  return glb({
    asset: { version: '2.0' },
    scene: 0,
    scenes: [{}],
    images: [{ uri: 'texture.png' }],
    samplers: [{ minFilter: 9987, magFilter: 9729 }],
    textures: [{ sampler: 0, source: 0 }],
    materials: [{ pbrMetallicRoughness: { baseColorTexture: { index: 0 } } }],
  });
}

beforeEach(() => {
  dependencies.createdBlobs.length = 0;
  dependencies.handlers.clear();
  dependencies.layerProps.length = 0;
  dependencies.revokedUrls.length = 0;
  dependencies.map.loaded.mockReturnValue(true);
  dependencies.map.getZoom.mockReturnValue(16);
  vi.stubGlobal('fetch', vi.fn(async () => jsonResponse(implicitTileset())));
  vi.spyOn(URL, 'createObjectURL').mockImplementation((value: Blob | MediaSource) => {
    if (!(value instanceof Blob)) throw new Error('expected a Blob manifest');
    dependencies.createdBlobs.push(value);
    return `blob:${dependencies.createdBlobs.length}`;
  });
  vi.spyOn(URL, 'revokeObjectURL').mockImplementation((url: string) => {
    dependencies.revokedUrls.push(url);
  });
});

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe('3D places loader contract', () => {
  it('provides visible zoom controls for the bounded 3D view', async () => {
    document.body.append(document.createElement('tellurion-places3d-panel'));
    await vi.waitFor(() => expect(dependencies.layerProps).toHaveLength(1));
    expect(dependencies.map.addControl).toHaveBeenCalledWith(
      expect.any(NavigationControl), 'top-right',
    );
  });

  it('parses the extensionless tileset root while keeping GLB content binary', async () => {
    document.body.append(document.createElement('tellurion-places3d-panel'));
    await vi.waitFor(() => expect(dependencies.layerProps).toHaveLength(1));

    const loadOptions = dependencies.layerProps[0].loadOptions as
      | { '3d-tiles'?: { isTileset?: boolean | 'auto' } }
      | undefined;
    const encoder = new TextEncoder();
    const root = {
      asset: { version: '1.1' },
      geometricError: 0,
      root: {
        boundingVolume: { box: [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1] },
        geometricError: 0,
        content: { uri: 'tile.glb' },
      },
    };
    const tileset = await parse(
      response(
        encoder.encode(JSON.stringify(root)),
        'https://tellurion.example/public/3dtiles/catalogs/catalog/collections/places/3dtiles',
      ),
      Tiles3DLoader,
      loadOptions,
    );

    expect(tileset).toEqual(expect.objectContaining({
      shape: 'tileset3d',
      root: expect.objectContaining({
        contentUrl: 'https://tellurion.example/public/3dtiles/catalogs/catalog/collections/places/tile.glb',
      }),
    }));

    const content = await parse(
      response(minimalGlb(), tileset.root.contentUrl),
      Tiles3DLoader,
      {
        ...loadOptions,
        '3d-tiles': { ...loadOptions?.['3d-tiles'], isTileset: false },
      },
    );
    expect(content).toEqual(expect.objectContaining({ shape: 'tile3d', type: 'glTF' }));
  });

  it('hands Tile3DLayer a private finite manifest instead of the implicit backend root', async () => {
    document.body.append(document.createElement('tellurion-places3d-panel'));
    await vi.waitFor(() => expect(dependencies.layerProps).toHaveLength(1));

    expect(fetch).toHaveBeenCalledWith(tilesetPath, expect.objectContaining({
      headers: { Accept: 'application/json' },
      signal: expect.any(AbortSignal),
    }));
    expect(dependencies.layerProps[0].data).toBe('blob:1');
    const manifest = JSON.parse(await dependencies.createdBlobs[0].text());
    expect(manifest.root).not.toHaveProperty('implicitTiling');
    expect(manifest.root.children).toHaveLength(49);
    expect(manifest.asset.gltfUpAxis).toBe('Z');

    const parsed = await parse(
      response(JSON.stringify(manifest), 'blob:http://localhost/finite-3d-tileset'),
      Tiles3DLoader,
      { '3d-tiles': { isTileset: true } },
    );
    expect(parsed.root.children).toHaveLength(49);
    expect(parsed.root.children[0]).toEqual(expect.objectContaining({
      contentUrl: `${window.location.origin}${tilesetPath}/tiles/16/23893/34116.glb`,
      transformMatrix: expect.any(Array),
    }));
  });

  it('refuses an over-budget view and asks the user to zoom closer', async () => {
    dependencies.map.getZoom.mockReturnValue(18);
    const element = document.createElement('tellurion-places3d-panel');
    document.body.append(element);

    await vi.waitFor(() => expect(element.textContent).toContain('zoom closer'));
    expect(dependencies.layerProps).toHaveLength(0);
    expect(dependencies.createdBlobs).toHaveLength(0);
    expect(dependencies.overlaySetProps).toHaveBeenLastCalledWith({ layers: [] });
  });

  it('keeps an explicit non-implicit tileset on its original URL', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonResponse({
      asset: { version: '1.1' },
      geometricError: 0,
      root: {
        boundingVolume: { box: [0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1] },
        geometricError: 0,
        content: { uri: `${window.location.origin}${tilesetPath}/tiles/0/0/0.glb` },
      },
    }));
    document.body.append(document.createElement('tellurion-places3d-panel'));

    await vi.waitFor(() => expect(dependencies.layerProps).toHaveLength(1));
    expect(dependencies.layerProps[0].data).toBe(tilesetPath);
    expect(dependencies.createdBlobs).toHaveLength(0);
  });

  it('clears malformed input without constructing a layer', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(jsonResponse({ asset: { version: '1.1' } }));
    const element = document.createElement('tellurion-places3d-panel');
    document.body.append(element);

    await vi.waitFor(() => expect(element.textContent).toContain('Invalid 3D tileset'));
    expect(dependencies.layerProps).toHaveLength(0);
    expect(dependencies.overlaySetProps).toHaveBeenLastCalledWith({ layers: [] });
  });

  it('revokes the previous manifest when the viewport changes and on disconnect', async () => {
    const element = document.createElement('tellurion-places3d-panel');
    document.body.append(element);
    await vi.waitFor(() => expect(dependencies.layerProps).toHaveLength(1));

    dependencies.handlers.get('moveend')?.();
    await vi.waitFor(() => expect(dependencies.layerProps).toHaveLength(2));
    expect(dependencies.revokedUrls).toContain('blob:1');

    element.remove();
    expect(dependencies.revokedUrls).toContain('blob:2');
    expect(dependencies.map.off).toHaveBeenCalledWith('moveend', expect.any(Function));
  });

  it('aborts an in-flight root request when disconnected', async () => {
    let requestSignal: AbortSignal | undefined;
    vi.mocked(fetch).mockImplementationOnce((_input, init) => {
      requestSignal = init?.signal ?? undefined;
      return new Promise<Response>(() => {});
    });
    const element = document.createElement('tellurion-places3d-panel');
    document.body.append(element);
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());

    element.remove();
    expect(requestSignal?.aborted).toBe(true);
  });

  it('ignores a stale root response after a viewport generation replaces it', async () => {
    let resolveFirst!: (response: Response) => void;
    let resolveSecond!: (response: Response) => void;
    const first = new Promise<Response>((resolve) => { resolveFirst = resolve; });
    const second = new Promise<Response>((resolve) => { resolveSecond = resolve; });
    vi.mocked(fetch)
      .mockImplementationOnce(() => first)
      .mockImplementationOnce(() => second);
    document.body.append(document.createElement('tellurion-places3d-panel'));
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());

    dependencies.handlers.get('moveend')?.();
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
    resolveSecond(jsonResponse(implicitTileset()));
    await vi.waitFor(() => expect(dependencies.layerProps).toHaveLength(1));

    resolveFirst(jsonResponse(implicitTileset()));
    await Promise.resolve();
    await Promise.resolve();
    expect(dependencies.layerProps).toHaveLength(1);
    expect(dependencies.createdBlobs).toHaveLength(1);
  });

  it('keeps textured GLB materials compatible with luma mipmap creation', async () => {
    const content = await parse(
      response(texturedGlb(), 'https://tellurion.example/public/3dtiles/tile.glb'),
      Tiles3DLoader,
      {
        '3d-tiles': { isTileset: false },
        gltf: { loadImages: false },
      },
    ) as {
      gltf: {
        materials: {
          pbrMetallicRoughness: {
            baseColorTexture: { texture: { source: { image: unknown } } };
          };
        }[];
      };
    };
    const decodedImage = { width: 4, height: 4 };
    const parsedMaterial = content.gltf.materials[0];
    parsedMaterial.pbrMetallicRoughness.baseColorTexture.texture.source.image = decodedImage;

    const generateMipmapsWebGL = vi.fn();
    const createdTexture = { generateMipmapsWebGL };
    const createTexture = vi.fn(() => createdTexture);
    const device = {
      type: 'webgl',
      getExternalImageSize: vi.fn(() => ({ width: 4, height: 4 })),
      getMipLevelCount: vi.fn(() => 3),
      createTexture,
    };

    const material = parsePBRMaterial(
      device as never,
      parsedMaterial as never,
      { NORMAL: {}, TEXCOORD_0: {} },
      { gltf: content.gltf as never },
    );

    expect(material.bindings.pbr_baseColorSampler).toBe(createdTexture);
    expect(createTexture).toHaveBeenCalledWith(expect.objectContaining({
      data: decodedImage,
      width: 4,
      height: 4,
      mipLevels: 3,
      sampler: expect.objectContaining({
        minFilter: 'linear',
        magFilter: 'linear',
        mipmapFilter: 'linear',
      }),
    }));
    expect(generateMipmapsWebGL).toHaveBeenCalledOnce();
  });
});
