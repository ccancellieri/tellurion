"""Guard live gallery URLs and Render inputs after the repository move."""

from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT.parents[1]
GALLERY_URL = "https://ccancellieri.github.io/tellurion/"
ITALY_URL = f"{GALLERY_URL}italy/"
ENGINE_RELEASE = "https://github.com/ccancellieri/tellurion/releases/download/archive-tellurion-${TELLURION_VERSION}"
ITALY_RELEASE = "https://github.com/ccancellieri/tellurion/releases/download/archive-italy-demo-${DEMO_VERSION}"


class MigrationWiringTests(unittest.TestCase):
    def test_live_pages_use_canonical_pages_and_archive_urls(self) -> None:
        pages = [
            ROOT / "index.html",
            ROOT / "docs/index.html",
            ROOT / "proof/index.html",
            ROOT / "italy/index.html",
            ROOT / "italy/sitemap.xml",
            ROOT / "italy/robots.txt",
            *ROOT.glob("demos/*/index.html"),
            *ROOT.glob("italy/articles/*/index.html"),
        ]
        self.assertIn(GALLERY_URL, (ROOT / "index.html").read_text())
        self.assertIn(ITALY_URL, (ROOT / "italy/index.html").read_text())
        for page in pages:
            with self.subTest(page=page.relative_to(ROOT)):
                content = page.read_text(encoding="utf-8")
                self.assertNotIn("ccancellieri.github.io/tellurion-demos/", content)
                self.assertNotIn("ccancellieri.github.io/tellurion-italy-demo/", content)
                self.assertNotIn("/tellurion-demos/releases/download/", content)
                self.assertNotIn("/tellurion-italy-demo/releases/download/", content)

    def test_active_docker_downloads_use_historical_archive_tags(self) -> None:
        dockerfiles = [*ROOT.glob("Dockerfile*"), ROOT / "italy/Dockerfile"]
        for dockerfile in dockerfiles:
            with self.subTest(dockerfile=dockerfile.relative_to(ROOT)):
                content = dockerfile.read_text(encoding="utf-8")
                if "TELLURION_RELEASE=" in content:
                    self.assertIn(ENGINE_RELEASE, content)
                if "DEMO_RELEASE=" in content:
                    self.assertIn(ITALY_RELEASE, content)

    def test_render_uses_canonical_repo_and_monorepo_contexts(self) -> None:
        import yaml

        gallery = yaml.safe_load((ROOT / "render.yaml").read_text())
        self.assertEqual(5, len(gallery["services"]))
        for service in gallery["services"]:
            with self.subTest(service=service["name"]):
                self.assertEqual("https://github.com/ccancellieri/tellurion", service["repo"])
                self.assertEqual("./demo/gallery", service["dockerContext"])
                self.assertTrue(service["dockerfilePath"].startswith("./demo/gallery/Dockerfile"))
                self.assertTrue((REPO / service["dockerfilePath"]).is_file())
                for path in service["buildFilter"]["paths"]:
                    self.assertTrue(path.startswith("demo/gallery/"), path)
                    self.assertTrue((REPO / path).exists(), path)

        italy = yaml.safe_load((ROOT / "italy/render.yaml").read_text())["services"]
        self.assertEqual(1, len(italy))
        self.assertEqual("https://github.com/ccancellieri/tellurion", italy[0]["repo"])
        self.assertEqual("./demo/gallery/italy", italy[0]["dockerContext"])
        self.assertEqual("./demo/gallery/italy/Dockerfile", italy[0]["dockerfilePath"])
        self.assertTrue((REPO / italy[0]["dockerfilePath"]).is_file())
        for path in italy[0]["buildFilter"]["paths"]:
            self.assertTrue(path.startswith("demo/gallery/italy/"), path)
            self.assertTrue((REPO / path).exists(), path)


if __name__ == "__main__":
    unittest.main()
