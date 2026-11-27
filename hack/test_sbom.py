import os, sys, unittest

sys.path.insert(0, os.path.dirname(__file__))
import sbom

BASE = {
    "spdxVersion": "SPDX-2.3",
    "packages": [
        {"SPDXID": "SPDXRef-Package-apk-hickory-filter-0.1.0-r0", "name": "hickory-filter", "versionInfo": "0.1.0-r0"},
        {"SPDXID": "SPDXRef-Package-apk-ca-certificates-bundle-1", "name": "ca-certificates-bundle", "versionInfo": "1"},
    ],
    "relationships": [],
}
NOTICES = {"crates": [
    {"name": "hickory-server", "version": "0.26.4", "license": "MIT OR Apache-2.0"},
    {"name": "old", "version": "1.0.0", "license": "MIT/Apache-2.0"},
    {"name": "bare", "version": "0.1.0", "license": ""},
    {"name": "musl", "version": "1.2.5-r1", "license": "MIT"},
]}


class Sbom(unittest.TestCase):
    def setUp(self):
        self.out = sbom.merge(BASE, NOTICES)
        self.pkgs = {p["name"]: p for p in self.out["packages"]}

    def test_keeps_the_image_packages_and_adds_the_crates(self):
        self.assertIn("ca-certificates-bundle", self.pkgs)
        self.assertIn("hickory-filter", self.pkgs)
        self.assertEqual(self.pkgs["hickory-server"]["versionInfo"], "0.26.4")

    def test_crates_have_a_cargo_purl_and_a_declared_license(self):
        ref = self.pkgs["hickory-server"]["externalRefs"][0]
        self.assertEqual(ref["referenceLocator"], "pkg:cargo/hickory-server@0.26.4")
        self.assertEqual(self.pkgs["hickory-server"]["licenseDeclared"], "MIT OR Apache-2.0")
        self.assertEqual(self.pkgs["old"]["licenseDeclared"], "MIT OR Apache-2.0")
        self.assertEqual(self.pkgs["bare"]["licenseDeclared"], "NOASSERTION")

    def test_the_binary_package_contains_every_crate(self):
        root = BASE["packages"][0]["SPDXID"]
        contained = {r["relatedSpdxElement"] for r in self.out["relationships"]
                     if r["spdxElementId"] == root and r["relationshipType"] == "CONTAINS"}
        self.assertEqual(contained, {p["SPDXID"] for n, p in self.pkgs.items() if p["SPDXID"].startswith("SPDXRef-Package-cargo-")})
        self.assertEqual(len(contained), 3)

    def test_the_c_library_is_left_to_the_notices(self):
        self.assertNotIn("musl", self.pkgs)

    def test_ids_are_unique_and_valid(self):
        ids = [p["SPDXID"] for p in self.out["packages"]]
        self.assertEqual(len(ids), len(set(ids)))
        for i in ids:
            self.assertRegex(i, r"^SPDXRef-[A-Za-z0-9.-]+$")

    def test_input_is_not_modified(self):
        self.assertEqual(len(BASE["packages"]), 2)

    def test_missing_binary_package_fails(self):
        with self.assertRaises(SystemExit):
            sbom.merge({"packages": [], "relationships": []}, NOTICES)


if __name__ == "__main__":
    unittest.main()
