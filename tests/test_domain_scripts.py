"""Offline regression tests for data-generation safeguards and metadata."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent


def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class DomainScripts(unittest.TestCase):
    def test_url_credentials_ports_and_ips(self):
        convert = load("convert_urls")
        self.assertEqual(convert.extract_domain("https://user:pass@example.com:443/file"), "example.com")
        self.assertIsNone(convert.extract_domain("https://[::1]/"))
        self.assertIsNone(convert.extract_domain("https://127.0.0.1/"))

    def test_missing_inputs_do_not_erase_community(self):
        convert = load("convert_urls")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "community.json"
            original = {"urlList": ["video.example.com"], "Analytics": ["analytics.example.com"]}
            path.write_text(json.dumps(original))
            with patch.object(convert, "URLS_DIR", Path(directory) / "missing"), patch.object(convert, "CONFIG_JSON_PATH", path):
                convert.main()
            self.assertEqual(json.loads(path.read_text()), original)
            convert.update_config_json({"image": ["images.example.com"]}, path)
            updated = json.loads(path.read_text())
            self.assertEqual(updated["urlList"], original["urlList"])
            self.assertEqual(updated["Analytics"], original["Analytics"])

    def test_invalid_config_is_not_replaced(self):
        convert = load("convert_urls")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "community.json"
            path.write_text("invalid JSON")
            with self.assertRaises(json.JSONDecodeError):
                convert.update_config_json({"video": []}, path)
            self.assertEqual(path.read_text(), "invalid JSON")

    def test_core_domains_and_parent_wildcards_are_protected(self):
        build = load("build_hosts_files")
        protected = build.PROTECTED_CORE_DOMAINS
        for domain in ("api.vrchat.cloud", "*.cloud", "*.com", "vrchat.net"):
            self.assertTrue(build.is_protected(domain, protected), domain)
        self.assertFalse(build.is_protected("example.com", protected))

    def test_bundled_lists_are_valid_and_unique(self):
        build = load("build_hosts_files")
        bundle = json.loads((ROOT / "assets/lists/domains.json").read_text())
        for category, domains in bundle.items():
            self.assertEqual(len(domains), len(set(domains)), category)
            for domain in domains:
                self.assertTrue(build.is_valid_domain_or_glob(domain), domain)
                self.assertFalse(build.is_protected(domain, build.PROTECTED_CORE_DOMAINS), domain)


if __name__ == "__main__":
    unittest.main()
