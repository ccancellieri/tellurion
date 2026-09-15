import unittest

from tests.canonical_links import LinkContractError, validate_items, validate_landing


VECTOR_BASE = "https://tellurion-vector-demo.onrender.com"
ITEMS_PATH = "/public/features/catalogs/default/collections/sample_roads/items"


class CanonicalLandingLinksTests(unittest.TestCase):
    def test_accepts_server_links_on_the_configured_origin(self):
        document = {
            "links": [
                {"rel": "self", "href": f"{VECTOR_BASE}/public"},
                {
                    "rel": "features",
                    "href": f"{VECTOR_BASE}/public/features/catalogs/default",
                },
            ]
        }

        validate_landing(document, VECTOR_BASE)

    def test_rejects_relative_server_links(self):
        document = {"links": [{"rel": "self", "href": "/public"}]}

        with self.assertRaisesRegex(LinkContractError, "absolute"):
            validate_landing(document, VECTOR_BASE)


class CanonicalItemsLinksTests(unittest.TestCase):
    def test_accepts_self_and_next_links_that_preserve_query_and_token(self):
        request_url = f"{VECTOR_BASE}{ITEMS_PATH}?filter=highway%20%3D%20%27primary%27&limit=1"
        document = {
            "links": [
                {
                    "rel": "self",
                    "href": f"{VECTOR_BASE}{ITEMS_PATH}?filter=highway%20%3D%20%27primary%27&limit=1",
                },
                {
                    "rel": "next",
                    "href": f"{VECTOR_BASE}{ITEMS_PATH}?filter=highway%20%3D%20%27primary%27&limit=1&token=4245316",
                },
            ]
        }

        validate_items(document, VECTOR_BASE, request_url)

    def test_rejects_a_next_link_that_drops_the_request_query(self):
        request_url = f"{VECTOR_BASE}{ITEMS_PATH}?filter=highway%20%3D%20%27primary%27&limit=1"
        document = {
            "links": [
                {"rel": "self", "href": request_url},
                {
                    "rel": "next",
                    "href": f"{VECTOR_BASE}{ITEMS_PATH}?limit=1&token=4245316",
                },
            ]
        }

        with self.assertRaisesRegex(LinkContractError, "preserve"):
            validate_items(document, VECTOR_BASE, request_url)

    def test_rejects_a_self_link_that_drops_the_request_query(self):
        request_url = f"{VECTOR_BASE}{ITEMS_PATH}?filter=highway%20%3D%20%27primary%27&limit=1"
        document = {
            "links": [
                {"rel": "self", "href": f"{VECTOR_BASE}{ITEMS_PATH}?limit=1"},
                {
                    "rel": "next",
                    "href": f"{VECTOR_BASE}{ITEMS_PATH}?filter=highway%20%3D%20%27primary%27&limit=1&token=4245316",
                },
            ]
        }

        with self.assertRaisesRegex(LinkContractError, "self.*query"):
            validate_items(document, VECTOR_BASE, request_url)

    def test_rejects_a_next_link_without_a_pagination_token(self):
        request_url = f"{VECTOR_BASE}{ITEMS_PATH}?limit=1"
        document = {
            "links": [
                {"rel": "self", "href": request_url},
                {"rel": "next", "href": request_url},
            ]
        }

        with self.assertRaisesRegex(LinkContractError, "token"):
            validate_items(document, VECTOR_BASE, request_url)

    def test_rejects_links_on_a_different_origin(self):
        request_url = f"{VECTOR_BASE}{ITEMS_PATH}?limit=1"
        document = {
            "links": [
                {
                    "rel": "self",
                    "href": f"https://example.invalid{ITEMS_PATH}?limit=1",
                },
                {
                    "rel": "next",
                    "href": f"https://example.invalid{ITEMS_PATH}?limit=1&token=1",
                },
            ]
        }

        with self.assertRaisesRegex(LinkContractError, "origin"):
            validate_items(document, VECTOR_BASE, request_url)


if __name__ == "__main__":
    unittest.main()
