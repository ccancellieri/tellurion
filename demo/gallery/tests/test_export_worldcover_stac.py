import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from scripts.export_worldcover_stac import ExportError, export_subset


class StaticSubsetTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / "source"
        (self.source / "items").mkdir(parents=True)
        self.collection = {
            "type": "Collection", "stac_version": "1.0.0", "id": "example",
            "description": "Synthetic metadata", "license": "CC-BY-4.0",
            "providers": [{"name": "Synthetic provider"}],
            "extent": {"spatial": {"bbox": [[-180, -90, 180, 90]]}, "temporal": {"interval": [[None, None]]}},
            "links": [{"rel": "self", "href": "https://example.test/collections/example/collection.json"}, {"rel": "items", "href": "items"}],
            "assets": {
                "overview": {"href": "overview.png", "roles": ["overview"]},
                "item-index": {"href": "abfs://items/example.parquet", "roles": ["stac-items"]},
            },
            "custom:metadata": {"preserve": [1, 2]},
        }
        self.items = []
        for index in range(2):
            self.items.append({
                "type": "Feature", "stac_version": "1.0.0", "id": ["tile-one", "../../tile-two"][index],
                "collection": "example", "bbox": [10 + index, 40, 11 + index, 41],
                "geometry": {"type": "Point", "coordinates": [10 + index, 40]},
                "properties": {"datetime": None, "start_datetime": f"202{index}-01-01T00:00:00Z", "end_datetime": f"202{index}-12-31T23:59:59Z", "proj:epsg": 4326},
                "stac_extensions": ["https://example.test/extension.json"],
                "links": [{"rel": "self", "href": f"https://example.test/items/{index}/item.json"}, {"rel": "collection", "href": "../../collection.json"}, {"rel": "license", "href": "license.txt"}],
                "assets": {"map": {"href": "map.tif", "type": "image/tiff", "raster:bands": [{"nodata": 0}], "custom:field": True}},
            })
        self.write_sources()

    def write_sources(self):
        (self.source / "collection.json").write_text(json.dumps(self.collection), encoding="utf-8")
        for index, item in enumerate(self.items):
            (self.source / "items" / f"{index}.json").write_text(json.dumps(item), encoding="utf-8")

    def test_portable_navigation_preserves_metadata_and_asset_locations(self):
        output = self.root / "export"
        export_subset(self.source, output, ["tile-one"])
        catalog = json.loads((output / "catalog.json").read_text())
        collection = json.loads((output / "collection.json").read_text())
        item_path = next((output / "items").glob("*.json"))
        item = json.loads(item_path.read_text())
        self.assertEqual(item["properties"], self.items[0]["properties"])
        self.assertEqual(item["geometry"], self.items[0]["geometry"])
        self.assertEqual(item["stac_extensions"], self.items[0]["stac_extensions"])
        self.assertEqual(item["assets"]["map"], {**self.items[0]["assets"]["map"], "href": "https://example.test/items/0/map.tif"})
        self.assertEqual(collection["custom:metadata"], self.collection["custom:metadata"])
        self.assertEqual(collection["providers"], self.collection["providers"])
        self.assertEqual(collection["license"], "CC-BY-4.0")
        self.assertEqual(collection["assets"]["overview"]["href"], "https://example.test/collections/example/overview.png")
        self.assertNotIn("item-index", collection["assets"])
        self.assertIn({"rel": "derived_from", "href": "https://example.test/items/0/item.json", "type": "application/geo+json"}, item["links"])
        for path, document in [(output / "catalog.json", catalog), (output / "collection.json", collection), (item_path, item)]:
            self.assertFalse(any(link["rel"] == "self" for link in document["links"]))
            for link in document["links"]:
                if link["rel"] in {"root", "parent", "child", "collection", "item"}:
                    target = (path.parent / link["href"]).resolve()
                    self.assertTrue(target.is_relative_to(output.resolve()))
                    self.assertTrue(target.is_file())

    def test_subset_extent_and_source_checksums_are_truthful(self):
        output = self.root / "export"
        export_subset(self.source, output, ["tile-one"])
        collection = json.loads((output / "collection.json").read_text())
        self.assertEqual(collection["extent"], {"spatial": {"bbox": [[10, 40, 11, 41]]}, "temporal": {"interval": [["2020-01-01T00:00:00Z", "2020-12-31T23:59:59Z"]]}})
        manifest = json.loads((output / "manifest.json").read_text())
        self.assertEqual(manifest["selected_item_ids"], ["tile-one"])
        self.assertEqual(manifest["omitted_collection_assets"], [{
            "key": "item-index", "reason": "replaced_by_exported_json_items",
        }])
        self.assertFalse(manifest["asset_bytes_downloaded"])
        self.assertEqual(manifest["source_sha256"]["items/0.json"], hashlib.sha256((self.source / "items/0.json").read_bytes()).hexdigest())
        self.assertNotIn(str(self.root), json.dumps(manifest))

    def test_all_items_are_sorted_safe_and_deterministic(self):
        first, second = self.root / "first", self.root / "second"
        export_subset(self.source, first)
        export_subset(self.source, second, ["tile-one", "../../tile-two"])
        def tree(root):
            return {str(path.relative_to(root)): path.read_bytes() for path in root.rglob("*") if path.is_file()}
        self.assertEqual(tree(first), tree(second))
        self.assertEqual(len(list((first / "items").glob("*.json"))), 2)
        self.assertFalse((self.root / "tile-two.json").exists())
        collection = json.loads((first / "collection.json").read_text())
        self.assertEqual(collection["extent"]["spatial"]["bbox"], [[10, 40, 12, 41]])

    def test_refuses_empty_unknown_or_duplicate_selection_before_writing(self):
        for selection in [[], ["missing"], ["tile-one", "tile-one"]]:
            with self.subTest(selection=selection):
                output = self.root / "export"
                with self.assertRaises(ExportError):
                    export_subset(self.source, output, selection)
                self.assertFalse(output.exists())

    def test_existing_output_is_not_overwritten(self):
        output = self.root / "export"
        output.mkdir()
        (output / "keep.txt").write_text("keep")
        with self.assertRaises(ExportError):
            export_subset(self.source, output)
        self.assertEqual((output / "keep.txt").read_text(), "keep")

    def test_missing_source_self_is_refused_without_guessing_asset_paths(self):
        self.items[0]["links"] = []
        self.write_sources()
        with self.assertRaises(ExportError):
            export_subset(self.source, self.root / "export", ["tile-one"])

    def test_unsafe_retained_hrefs_are_refused_before_writing(self):
        original_href = self.items[0]["assets"]["map"]["href"]
        self.items[0]["assets"]["map"]["href"] = "javascript:alert(1)"
        self.write_sources()
        unsafe_scheme = self.root / "unsafe-scheme"
        with self.assertRaises(ExportError):
            export_subset(self.source, unsafe_scheme, ["tile-one"])
        self.assertFalse(unsafe_scheme.exists())

        self.items[0]["assets"]["map"]["href"] = original_href
        self.items[0]["links"].append({
            "rel": "license", "href": "https://user:secret@invalid/license",
        })
        self.write_sources()
        credentials = self.root / "credentials"
        with self.assertRaises(ExportError):
            export_subset(self.source, credentials, ["tile-one"])
        self.assertFalse(credentials.exists())

        self.items[0]["links"].pop()
        self.collection["assets"]["ambiguous-index"] = {
            "href": "abfs://items/example.parquet", "roles": ["stac-items", "data"],
        }
        self.write_sources()
        ambiguous_roles = self.root / "ambiguous-roles"
        with self.assertRaises(ExportError):
            export_subset(self.source, ambiguous_roles, ["tile-one"])
        self.assertFalse(ambiguous_roles.exists())

    def test_invalid_bbox_or_time_is_refused_before_writing(self):
        for key, value in [("bbox", [10, 40, 9, 41]), ("properties", {"datetime": "not-a-date"})]:
            original = self.items[0][key]
            self.items[0][key] = value
            self.write_sources()
            with self.assertRaises(ExportError):
                export_subset(self.source, self.root / "export", ["tile-one"])
            self.items[0][key] = original
            self.assertFalse((self.root / "export").exists())


if __name__ == "__main__":
    unittest.main()
