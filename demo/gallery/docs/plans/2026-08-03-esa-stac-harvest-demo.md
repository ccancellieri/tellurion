# ESA STAC Harvest Demo Implementation Plan

> **For implementers:** Execute this plan task by task. Review the diff and
> verification evidence at every commit boundary before continuing.

**Goal:** Publish a reproducible ESA WorldCover STAC harvest whose source COG,
classification colors, provenance, Tellurion STAC projection, and dynamically
rendered raster tiles can all be inspected from one public demo.

**Architecture:** A standard-library Python harvester snapshots one pinned
Collection and Item during build/release, validates the source contract, and
derives a one-feature GeoJSON fixture, an explicit-stop Tellurion colormap, a
legend, and a provenance manifest. A dedicated Render image creates a
GeoPackage for the Features/STAC lane and routes raster Tiles to the harvested
COG. The static viewer reads committed evidence and live Tellurion endpoints;
it never calls the upstream catalog at runtime.

**Tech Stack:** Python 3 standard library and `unittest`, POSIX shell, GDAL
`ogr2ogr`, Tellurion 0.3.0, GeoPackage, COG, Render Blueprint, MapLibre GL JS,
GitHub Actions.

## Global Constraints

- Source Collection is `esa-worldcover`.
- Source Item is `ESA_WorldCover_10m_2021_v200_N39E012`.
- Search bbox is `12.45,41.87,12.55,41.95`; datetime is calendar year 2021.
- Source snapshots are immutable evidence; derived files record their digest.
- Runtime requests never contact the upstream STAC API.
- No credentials, signed URLs, SAS tokens, or expiring URLs are committed.
- Only `classification:classes` integer values and six-digit `color-hint`
  strings are translated; arbitrary expressions and remote styles are never
  executed.
- The GeoPackage uses deterministic local integer id `1`; the upstream Item id
  is preserved in `source_item_id`.
- The public service is read-only and runs as UID/GID `10001`.
- Do not claim raster OGC API Maps support; the first slice proves STAC,
  Features, raster TileSet metadata, and PNG Tiles.
- Preserve ESA, ESA WorldCover Consortium, Microsoft host, and CC BY 4.0
  attribution on every public surface.

---

### Task 1: Offline harvest contract and failure tests

**Files:**

- Create: `scripts/harvest_esa_worldcover.py`
- Create: `tests/test_harvest_esa_worldcover.py`
- Create: `tests/fixtures/stac/collection.json`
- Create: `tests/fixtures/stac/item-search.json`
- Modify: `.github/workflows/verify.yml`

**Interfaces:**

- Consumes: a STAC Collection object and one STAC Item Search FeatureCollection.
- Produces: `validate_and_derive(collection: dict, search: dict) -> Derived`.
- `Derived` carries `collection`, `item`, `footprint`, `legend`, `colormap`, and
  manifest facts before serialization.

- [ ] **Step 1: Add minimized, semantically complete local fixtures**

Keep the real collection/item ids, bbox, geometry, license, providers, map
asset, media type, raster band, all eleven `classification:classes`, projection
fields, start/end datetimes, and source links. Remove unrelated provider UI
fields so test intent stays visible.

- [ ] **Step 2: Write the success and failure tests**

```python
import copy
import json
import unittest
from pathlib import Path

from scripts.harvest_esa_worldcover import HarvestError, validate_and_derive

FIXTURES = Path(__file__).parent / "fixtures" / "stac"


def load(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


class HarvestContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.collection = load("collection.json")
        self.search = load("item-search.json")

    def test_derives_exact_worldcover_palette_and_source_identity(self):
        result = validate_and_derive(self.collection, self.search)
        self.assertEqual(result.item["id"], "ESA_WorldCover_10m_2021_v200_N39E012")
        self.assertEqual(result.footprint["features"][0]["id"], 1)
        self.assertEqual(
            result.footprint["features"][0]["properties"]["source_item_id"],
            result.item["id"],
        )
        self.assertEqual(
            [entry["value"] for entry in result.legend],
            [10, 20, 30, 40, 50, 60, 70, 80, 90, 95, 100],
        )
        self.assertEqual(result.legend[0]["color"], "#006400")

    def test_rejects_an_unexpected_item(self):
        self.search["features"][0]["id"] = "another-item"
        with self.assertRaisesRegex(HarvestError, "unexpected Item"):
            validate_and_derive(self.collection, self.search)

    def test_rejects_a_duplicate_class_value(self):
        classes = self.search["features"][0]["assets"]["map"]["classification:classes"]
        classes[1]["value"] = classes[0]["value"]
        with self.assertRaisesRegex(HarvestError, "duplicate class value"):
            validate_and_derive(self.collection, self.search)

    def test_rejects_a_malformed_color(self):
        classes = self.search["features"][0]["assets"]["map"]["classification:classes"]
        classes[0]["color-hint"] = "green"
        with self.assertRaisesRegex(HarvestError, "color-hint"):
            validate_and_derive(self.collection, self.search)

    def test_rejects_a_non_cog_map_asset(self):
        self.search["features"][0]["assets"]["map"]["type"] = "image/tiff"
        with self.assertRaisesRegex(HarvestError, "Cloud Optimized GeoTIFF"):
            validate_and_derive(self.collection, self.search)
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `python3 -m unittest -v tests.test_harvest_esa_worldcover`

Expected: import failure because `scripts/harvest_esa_worldcover.py` does not
exist yet.

- [ ] **Step 4: Add the minimal public types and validation skeleton**

```python
@dataclass(frozen=True)
class Derived:
    collection: dict
    item: dict
    footprint: dict
    legend: list[dict]
    colormap: dict
    manifest_facts: dict


class HarvestError(ValueError):
    pass


def validate_and_derive(collection: dict, search: dict) -> Derived:
    if collection.get("id") != COLLECTION_ID:
        raise HarvestError("unexpected Collection")
    if collection.get("license") != "CC-BY-4.0":
        raise HarvestError("expected CC-BY-4.0 license")
    providers = collection.get("providers") or []
    names = {provider.get("name") for provider in providers}
    if "ESA" not in names or "Microsoft" not in names:
        raise HarvestError("expected ESA producer and Microsoft host")

    features = search.get("features") or []
    if len(features) != 1 or features[0].get("id") != ITEM_ID:
        raise HarvestError("unexpected Item")
    item = features[0]
    asset = (item.get("assets") or {}).get("map")
    if not isinstance(asset, dict):
        raise HarvestError("missing map asset")
    media_type = asset.get("type", "")
    if "profile=cloud-optimized" not in media_type:
        raise HarvestError("map asset is not a Cloud Optimized GeoTIFF")
    href = asset.get("href", "")
    parsed = urlsplit(href)
    if parsed.scheme != "https" or parsed.query:
        raise HarvestError("map asset must be an unsigned public HTTPS URL")

    classes = asset.get("classification:classes") or []
    values = [entry.get("value") for entry in classes]
    if values != EXPECTED_VALUES:
        if len(values) != len(set(values)):
            raise HarvestError("duplicate class value")
        raise HarvestError("unexpected classification values")
    legend = []
    for entry in classes:
        color = entry.get("color-hint", "")
        label = entry.get("description", "")
        if not re.fullmatch(r"[0-9A-Fa-f]{6}", color):
            raise HarvestError("invalid color-hint")
        if not label:
            raise HarvestError("classification description is required")
        legend.append({"value": entry["value"], "label": label, "color": f"#{color.upper()}"})

    properties = item.get("properties") or {}
    footprint = {
        "type": "FeatureCollection",
        "features": [{
            "type": "Feature",
            "id": 1,
            "geometry": item.get("geometry"),
            "properties": {
                "id": 1,
                "source_item_id": ITEM_ID,
                "start_datetime": properties.get("start_datetime"),
                "end_datetime": properties.get("end_datetime"),
                "product_version": properties.get("esa_worldcover:product_version"),
                "grid_code": properties.get("grid:code"),
            },
        }],
    }
    stops = [{"value": 0.0, "rgba": [0, 0, 0, 0]}]
    for entry in legend:
        color = entry["color"]
        stops.append({
            "value": float(entry["value"]),
            "rgba": [int(color[1:3], 16), int(color[3:5], 16), int(color[5:7], 16), 255],
        })
    return Derived(
        collection=collection,
        item=item,
        footprint=footprint,
        legend=legend,
        colormap={"kind": "stops", "stops": stops},
        manifest_facts={"collection_id": COLLECTION_ID, "item_id": ITEM_ID, "asset_href": href},
    )
```

Define `COLLECTION_ID`, `ITEM_ID`, `EXPECTED_VALUES`, the `Derived` dataclass,
and imports in the same module. Do not introduce schema libraries or
provider-specific SDKs.

- [ ] **Step 5: Add the unit test job to CI**

Add before deployment contracts:

```yaml
      - name: Verify STAC harvest transformation
        run: python3 -m unittest -v tests.test_harvest_esa_worldcover
```

- [ ] **Step 6: Run the focused tests**

Run: `python3 -m unittest -v tests.test_harvest_esa_worldcover`

Expected: all five tests pass.

- [ ] **Step 7: Commit**

```bash
git add scripts/harvest_esa_worldcover.py tests/test_harvest_esa_worldcover.py tests/fixtures/stac .github/workflows/verify.yml
git commit -m "Add the ESA WorldCover harvest contract"
```

---

### Task 2: Network refresh command and deterministic derived artifacts

**Files:**

- Modify: `scripts/harvest_esa_worldcover.py`
- Modify: `tests/test_harvest_esa_worldcover.py`
- Create: `data/stac/esa-worldcover/collection.json`
- Create: `data/stac/esa-worldcover/item.json`
- Create: `data/stac/esa-worldcover/manifest.json`
- Create: `data/stac/esa-worldcover/footprint.geojson`
- Create: `data/stac/esa-worldcover/legend.json`
- Create: `data/stac/esa-worldcover/colormap.yaml`

**Interfaces:**

- `fetch_json(url: str, params: Mapping[str, str] | None = None) -> dict`
- `write_artifacts(derived: Derived, output: Path, retrieved_at: str) -> None`
- CLI: `python3 scripts/harvest_esa_worldcover.py OUTPUT [--fixture-dir DIR]
  [--retrieved-at ISO8601]`

- [ ] **Step 1: Test deterministic serialization**

```python
def test_writes_digest_linked_deterministic_outputs(self):
    with tempfile.TemporaryDirectory() as directory:
        output = Path(directory)
        derived = validate_and_derive(self.collection, self.search)
        write_artifacts(derived, output, "2026-08-03T00:00:00Z")
        manifest = json.loads((output / "manifest.json").read_text())
        legend = json.loads((output / "legend.json").read_text())
        self.assertEqual(manifest["source"]["item_id"], derived.item["id"])
        self.assertRegex(manifest["digests"]["item"], r"^[0-9a-f]{64}$")
        self.assertEqual(legend[4], {"value": 50, "label": "Built-up", "color": "#FA0000"})
        self.assertTrue((output / "colormap.yaml").read_text().endswith("\n"))
```

- [ ] **Step 2: Implement URL encoding, fetch, and atomic artifact writes**

Use `urllib.parse.urlencode`, `urllib.request.urlopen` with a 30-second timeout,
`hashlib.sha256`, `json.dumps(indent=2, sort_keys=True)`, temporary sibling
files, and `Path.replace`. The CLI's fixture mode reads the two local inputs and
must produce identical semantic outputs without network access.

The colormap YAML must have this exact shape:

```yaml
kind: stops
stops:
  - { value: 0.0, rgba: [0, 0, 0, 0] }
  - { value: 10.0, rgba: [0, 100, 0, 255] }
```

Continue through all eleven classes in ascending numeric order.

- [ ] **Step 3: Verify fixture-mode output twice**

Run:

```bash
first=$(mktemp -d)
second=$(mktemp -d)
python3 scripts/harvest_esa_worldcover.py "$first" --fixture-dir tests/fixtures/stac --retrieved-at 2026-08-03T00:00:00Z
python3 scripts/harvest_esa_worldcover.py "$second" --fixture-dir tests/fixtures/stac --retrieved-at 2026-08-03T00:00:00Z
diff -ru "$first" "$second"
```

Expected: no diff.

- [ ] **Step 4: Refresh the pinned real snapshot**

Run:

```bash
python3 scripts/harvest_esa_worldcover.py data/stac/esa-worldcover
```

Inspect the diff for stable source ids, public unsigned COG URL, attribution,
license, exact class table, and absence of tokens or secrets.

- [ ] **Step 5: Run unit tests and JSON parsing checks**

Run:

```bash
python3 -m unittest -v tests.test_harvest_esa_worldcover
for file in data/stac/esa-worldcover/*.json data/stac/esa-worldcover/*.geojson; do python3 -m json.tool "$file" >/dev/null; done
```

Expected: all tests and parses pass.

- [ ] **Step 6: Commit**

```bash
git add scripts/harvest_esa_worldcover.py tests/test_harvest_esa_worldcover.py data/stac/esa-worldcover
git commit -m "Harvest the bounded ESA WorldCover STAC item"
```

---

### Task 3: Split-lane fixture and read-only Render service

**Files:**

- Create: `Dockerfile.stac-harvest`
- Create: `deploy/render/stac-harvest.yaml`
- Create: `tests/render_stac_harvest_contract.sh`
- Modify: `render.yaml`
- Modify: `.github/workflows/verify.yml`

**Interfaces:**

- `TELLURION_GEOPACKAGE_PATH=/app/data/worldcover.gpkg`
- `TELLURION_COG_URL=<public unsigned source COG URL>`
- Collection id: `esa_worldcover_2021_rome`
- GeoPackage table: `esa_worldcover_2021_rome`
- Feature local id: `1`; property `source_item_id` holds the upstream id.

- [ ] **Step 1: Write a failing deployment contract**

The shell contract must require the new files and assert:

```sh
require_text Dockerfile.stac-harvest 'ARG TELLURION_VERSION=v0.3.0'
require_text Dockerfile.stac-harvest 'sha256sum -c'
require_text Dockerfile.stac-harvest 'ogr2ogr -f GPKG'
require_text Dockerfile.stac-harvest 'USER 10001:10001'
require_text deploy/render/stac-harvest.yaml 'features: harvested_items'
require_text deploy/render/stac-harvest.yaml 'tiles: harvested_cog'
require_text deploy/render/stac-harvest.yaml 'source_item_id'
require_text render.yaml 'name: tellurion-stac-harvest-demo'

if grep -Eq 'write:[[:space:]]' "$ROOT/deploy/render/stac-harvest.yaml"; then
  printf 'the STAC harvest demo must not configure a write route\n' >&2
  exit 1
fi
```

- [ ] **Step 2: Run the contract to verify it fails**

Run: `sh tests/render_stac_harvest_contract.sh`

Expected: missing deployment file.

- [ ] **Step 3: Add the multi-stage Docker build**

Follow `Dockerfile.raster`'s pinned source archive and SHA-256 verification.
The data stage must:

```dockerfile
COPY data/stac/esa-worldcover /build/stac
RUN mkdir -p /app/data \
    && ogr2ogr -f GPKG /app/data/worldcover.gpkg /build/stac/footprint.geojson \
         -nln esa_worldcover_2021_rome \
         -lco GEOMETRY_NAME=geom \
         -lco FID=id \
    && test "$(ogrinfo -ro -so /app/data/worldcover.gpkg esa_worldcover_2021_rome | sed -n 's/^Feature Count: //p')" = "1" \
    && chmod 0555 /app/data \
    && chmod 0444 /app/data/worldcover.gpkg
```

Set the image environment value to the exact public unsigned COG URL recorded
in the committed manifest:

```dockerfile
ENV TELLURION_COG_URL=https://ai4edataeuwest.blob.core.windows.net/esa-worldcover/v200/2021/map/ESA_WorldCover_10m_2021_v200_N39E012_Map.tif
```

The deployment contract must parse `manifest.json`, compare its asset href to
this value, and reject `?`, `sig=`, `token=`, and `se=`. Do not download the
3-degree global tile into the image.

- [ ] **Step 4: Add split-lane Tellurion configuration**

```yaml
storages:
  - id: harvested_items
    driver: geopackage
    url_env: TELLURION_GEOPACKAGE_PATH
  - id: harvested_cog
    driver: cog
    url_env: TELLURION_COG_URL

collections:
  - id: esa_worldcover_2021_rome
    catalog: default
    storage: harvested_items
    routing:
      features: harvested_items
      tiles: harvested_cog
    settings:
      colormap:
        kind: stops
        stops:
          - { value: 0.0, rgba: [0, 0, 0, 0] }
          - { value: 10.0, rgba: [0, 100, 0, 255] }
          - { value: 20.0, rgba: [255, 187, 34, 255] }
          - { value: 30.0, rgba: [255, 255, 76, 255] }
          - { value: 40.0, rgba: [240, 150, 255, 255] }
          - { value: 50.0, rgba: [250, 0, 0, 255] }
          - { value: 60.0, rgba: [180, 180, 180, 255] }
          - { value: 70.0, rgba: [240, 240, 240, 255] }
          - { value: 80.0, rgba: [0, 100, 200, 255] }
          - { value: 90.0, rgba: [0, 150, 160, 255] }
          - { value: 95.0, rgba: [0, 207, 117, 255] }
          - { value: 100.0, rgba: [250, 230, 160, 255] }
```

Declare `source_item_id`, `start_datetime`, `end_datetime`, `product_version`,
and `grid_code` in the collection schema. Add source COG, source snapshot, and
provenance manifest as declared STAC assets with truthful media types and roles.

- [ ] **Step 5: Add the Render service and CI contract**

Add `tellurion-stac-harvest-demo` in `render.yaml`, using the same region,
free plan, root health check, `checksPass`, and build-filter conventions as the
other services. Run the new shell contract from `verify.yml`.

- [ ] **Step 6: Run offline verification**

Run:

```bash
sh tests/render_stac_harvest_contract.sh
docker build -f Dockerfile.stac-harvest -t tellurion-stac-harvest:test .
```

Then run the container locally and verify `/`, the STAC Collection and Item,
the Features Item, raster TileSet metadata, and one Rome PNG tile.

- [ ] **Step 7: Commit**

```bash
git add Dockerfile.stac-harvest deploy/render/stac-harvest.yaml tests/render_stac_harvest_contract.sh render.yaml .github/workflows/verify.yml
git commit -m "Deploy the harvested WorldCover collection"
```

---

### Task 4: Harvest viewer and generated legend

**Files:**

- Modify: `demos/stac/index.html`
- Modify: `demos/protocol.css`
- Modify: `demos/viewer.css`
- Modify: `index.html`
- Modify: `README.md`
- Modify: `tests/render_protocol_gallery_contract.sh`
- Copy: `data/stac/esa-worldcover/legend.json` to `demos/stac/legend.json`
- Copy: `data/stac/esa-worldcover/manifest.json` to `demos/stac/manifest.json`

**Interfaces:**

- Browser service base: `https://tellurion-stac-harvest-demo.onrender.com`
- Collection: `/public/stac/catalogs/default/collections/esa_worldcover_2021_rome`
- Item: same path plus `/items/1`
- TileSet: `/public/tiles/catalogs/default/collections/esa_worldcover_2021_rome/tiles/WebMercatorQuad`
- PNG template: TileSet path plus `/{z}/{y}/{x}.png`

- [ ] **Step 1: Update the gallery contract first**

Require the new service base, `source_item_id`, `legend.json`, `manifest.json`,
the source/derived distinction, ESA/Microsoft/CC BY 4.0 attribution, and all
four live resource URLs. Remove the old assertion that this is not an asset
catalog demonstration.

- [ ] **Step 2: Run the gallery contract to verify it fails**

Run: `sh tests/render_protocol_gallery_contract.sh`

Expected: required harvest copy and resources are absent.

- [ ] **Step 3: Implement the map and evidence panels**

Use one MapLibre raster source with the Tellurion PNG template. Load local
`legend.json` and `manifest.json` using `Promise.all`, then fetch the four live
Tellurion resources independently with per-resource status. Render legend rows
with DOM construction and `textContent`; do not interpolate harvested strings
into `innerHTML`.

The resource graph must show:

```text
Planetary Computer STAC snapshot
  -> ESA source COG asset
  -> Tellurion COG driver
  -> raster TileSet / PNG tiles
  -> browser map
```

- [ ] **Step 4: Update gallery and README copy**

Describe the STAC lane as “ESA WorldCover STAC → preserved COG and class
metadata → Tellurion STAC/Features/raster Tiles”. State that the map is dynamic
PNG tile composition, not a raster OGC API Maps conformance claim.

- [ ] **Step 5: Verify static serving and responsive layout**

Run the repository HTTP server, open the gallery and STAC page, and inspect at
desktop and mobile widths. Verify focus order, map fallback, attribution,
contrast, and cold-start status.

- [ ] **Step 6: Run contracts and commit**

```bash
sh tests/render_protocol_gallery_contract.sh
git add demos/stac demos/protocol.css demos/viewer.css index.html README.md tests/render_protocol_gallery_contract.sh
git commit -m "Show the harvested STAC-to-map path"
```

---

### Task 5: Live smoke tests and publication evidence

**Files:**

- Modify: `.github/workflows/smoke.yml`
- Create: `evidence/stac-harvest/endpoints.md`
- Create after deployment: `evidence/stac-harvest/desktop.png`
- Create after deployment: `evidence/stac-harvest/mobile.png`

**Interfaces:** Same four public endpoints defined in Task 4.

- [ ] **Step 1: Add four smoke matrix entries**

Verify:

- STAC Collection: `application/json`, minimum 500 bytes;
- STAC Item: `application/geo+json`, minimum 500 bytes;
- raster TileSet: `application/json`, minimum 300 bytes;
- representative PNG: `image/png`, minimum 1,000 bytes.

- [ ] **Step 2: Run every offline gate**

```bash
python3 -m unittest -v tests.test_harvest_esa_worldcover
sh tests/render_deployment_contract.sh
sh tests/render_raster_contract.sh
sh tests/render_zarr_contract.sh
sh tests/render_3d_contract.sh
sh tests/render_protocol_gallery_contract.sh
sh tests/render_stac_harvest_contract.sh
```

- [ ] **Step 3: Push through a review branch and deploy**

Push the commits to one focused branch, open a draft PR linked to demo issues
#3–#8, wait for CI, review the rendered diff, then merge only when green.

- [ ] **Step 4: Verify the live service**

Record UTC time, deployed revision, endpoint, status, content type, byte size,
and semantic assertions in `evidence/stac-harvest/endpoints.md`. Do not record
latency as performance evidence.

- [ ] **Step 5: Capture visual evidence**

Capture desktop and mobile screenshots after the service is warm. Confirm the
legend colors against the committed source metadata and the map visually.

- [ ] **Step 6: Commit evidence and close implementation issues**

```bash
git add .github/workflows/smoke.yml evidence/stac-harvest
git commit -m "Record the live STAC harvest evidence"
```

Close #4–#8 only when each issue's acceptance criteria is proven. Keep the epic
open until the article task is complete.

---

### Task 6: Results article and portfolio launch updates

**Files:**

- Create: `docs/articles/from-stac-discovery-to-a-live-map.md`
- Modify: `README.md`
- Modify: `../ccancellieri.github.io/index.html`
- Modify: `../ccancellieri.github.io/sitemap.xml`
- Modify when appropriate: `../portfolio/index.html`

**Interfaces:** Consumes only verified links, screenshots, source attribution,
and evidence recorded in Task 5.

- [ ] **Step 1: Draft the article from verified facts**

Use this fixed structure:

1. visible result and screenshot;
2. one source Item and why it was selected;
3. immutable snapshot and provenance;
4. COG asset versus API links;
5. eleven-class color translation;
6. split Features/raster routing;
7. live STAC, TileSet, and PNG resources;
8. limitations and reproducibility.

End with: “Interoperability is not copying JSON; it is preserving meaning while
each system adds only the capabilities it can prove.”

- [ ] **Step 2: Prepare the LinkedIn post**

```markdown
I wanted to test a simple question: can a STAC record keep its meaning all the
way from discovery to a live, dynamically rendered map?

I harvested one bounded ESA WorldCover Item covering Rome. The source STAC
metadata already described the public Cloud Optimized GeoTIFF and the exact
colors for its eleven land-cover classes.

The demo preserves that source document and provenance, translates the class
metadata into a validated Tellurion colormap, routes the Item footprint through
GeoPackage and the raster data through the COG driver, then exposes the result
through STAC, OGC API Features, raster TileSet metadata, and live PNG tiles.

The important boundary: the COG is an asset; the dynamic APIs are links. The
harvest happens during release, not while serving requests, and unsupported
remote style instructions are never executed.

Live demo: https://ccancellieri.github.io/tellurion-demos/demos/stac/
Source and reproducibility: https://github.com/ccancellieri/tellurion-demos

#STAC #OGCAPI #CloudOptimizedGeoTIFF #Rust #Geospatial #OpenStandards
```

Publish this copy only after Task 5 verifies both URLs and the live resources
linked by the demo page.

- [ ] **Step 3: Add the verified result to public portfolio surfaces**

Use architecture-based wording. Do not describe hosted request volume, user
count, production scale, or benchmark improvements. Link only GitHub, LinkedIn,
the public demo, and article.

- [ ] **Step 4: Verify public pages and commit per repository**

Run each repository's existing static checks. Commit Tellurion demo article,
public GitHub Pages changes, and portfolio changes separately so each can be
reviewed or reverted independently.

- [ ] **Step 5: Close launch tracking**

Close demo issue #9 and the epic #3 with links to the live demo, evidence,
article, and final revisions. Leave Tellurion #37, #220, and #221 open until
their generalized engine contracts land.
