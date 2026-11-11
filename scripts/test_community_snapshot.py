"""Offline checks for misleading counts, partial APIs and public-only reporting."""
import importlib.util
from pathlib import Path
import unittest
import urllib.error

spec = importlib.util.spec_from_file_location("snapshot", Path(__file__).with_name("community-snapshot.py"))
snapshot = importlib.util.module_from_spec(spec)
spec.loader.exec_module(snapshot)


def response(url):
    if "reverse_dependencies" in url:
        return {"meta": {"total": 101}, "versions": [{"crate": "phronesis-mcp", "num": "0.41.1"}, {"crate": "external-example", "num": "1.0"}]}
    if "crates.io" in url:
        return {"crate": {"downloads": 42, "recent_downloads": 7, "max_version": "0.41.1"}}
    if "/forks?" in url:
        return [{"private": False, "full_name": "example/phronesis", "html_url": "https://github.com/example/phronesis", "created_at": "2026-11-11"},
                {"private": True, "full_name": "private/secret"}]
    if "api.github.com/search/" in url:
        return {"total_count": 101, "incomplete_results": True,
                "items": [{"html_url": "https://github.com/example/public", "title": "<script> [notice]", "body": "not retained", "repository": {"private": False}},
                          {"html_url": "https://github.com/private/secret", "repository": {"private": True}}]}
    if "hn.algolia.com" in url:
        return {"nbHits": 0, "hits": []}
    return {"stargazers_count": 1, "forks_count": 1, "subscribers_count": 0}


class SnapshotTests(unittest.TestCase):
    def test_partial_api_is_unknown_not_zero_and_other_sources_survive(self):
        def get(url):
            if "search/code" in url:
                raise urllib.error.HTTPError(url, 403, "credential must not leak", {}, None)
            return response(url)
        data = snapshot.collect(get)
        self.assertEqual(data["status"], "partial")
        self.assertEqual(len(data["errors"]), 3)
        self.assertNotIn('github:code:"phr-mcp"', data["sources"])
        self.assertEqual(data["sources"]["crate:phronesis"]["data"]["downloads"], 42)
        rendered = snapshot.summary(data)
        self.assertIn("unknown, not zero", rendered)
        self.assertNotIn("credential must not leak", rendered)

    def test_companions_caps_and_untrusted_labels_are_explicit(self):
        data = snapshot.collect(response)
        self.assertEqual(data["status"], "complete")
        deps = data["sources"]["dependents:phronesis"]["data"]
        self.assertTrue(deps["truncated"])
        self.assertEqual([x["companion"] for x in deps["crates"]], [True, False])
        rendered = snapshot.summary(data)
        self.assertIn("not confirmed users", rendered)
        self.assertIn("Partial source results", rendered)
        self.assertNotIn("<script>", rendered)
        self.assertNotIn("private/secret", rendered)
        self.assertNotIn("not retained", rendered)
        self.assertIn("&lt;script&gt;", rendered)

    def test_query_scope_excludes_private_and_first_party_repositories(self):
        seen = []
        def get(url):
            seen.append(url)
            return response(url)
        snapshot.collect(get)
        from urllib.parse import parse_qs, urlparse
        for url in seen:
            if "api.github.com/search/" in url:
                query = parse_qs(urlparse(url).query)["q"][0]
                if "/search/issues" in url:
                    self.assertIn("is:public", query)
                else:
                    self.assertNotIn("is:public", query)
                self.assertIn("-user:awaterma", query)
        self.assertFalse(any("/traffic" in url for url in seen))


if __name__ == "__main__":
    unittest.main()
