import os, sys, tempfile, unittest

sys.path.insert(0, os.path.dirname(__file__))
import notices


def package(root, name, version, license_, files=None, source="registry+x"):
    d = os.path.join(root, f"{name}-{version}")
    os.makedirs(d, exist_ok=True)
    for fname, text in (files or {}).items():
        open(os.path.join(d, fname), "w").write(text)
    return {"id": f"{name} {version}", "name": name, "version": version, "license": license_, "source": source,
            "manifest_path": os.path.join(d, "Cargo.toml")}


def metadata(pkgs, edges):
    """edges: {id: [(dep id, kind)]} with kind None, 'build' or 'dev'."""
    ids = [p["id"] for p in pkgs]
    nodes = [{"id": i, "deps": [{"pkg": d, "dep_kinds": [{"kind": k}]} for d, k in edges.get(i, [])]} for i in ids]
    return {"packages": pkgs, "resolve": {"root": "app 1.0.0", "nodes": nodes}}


class Notices(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = self.tmp.name

    def build(self, pkgs, edges, exceptions=None):
        pkgs = [package(self.root, "app", "1.0.0", "MIT")] + pkgs
        return notices.build_entries(metadata(pkgs, edges), notices.SPDX_DIR, exceptions)

    def test_crate_with_notice_file_has_all_files(self):
        dep = package(self.root, "dep", "1.0.0", "Apache-2.0",
                      {"LICENSE-APACHE": "apache text", "NOTICE": "notice text", "main.rs": "x"})
        [e] = self.build([dep], {"app 1.0.0": [("dep 1.0.0", None)]})
        self.assertEqual([f["name"] for f in e["files"]], ["LICENSE-APACHE", "NOTICE"])
        self.assertEqual(e["files"][1]["text"], "notice text")
        self.assertNotIn("fallback", e)

    def test_crate_without_files_gets_canonical_text(self):
        dep = package(self.root, "dep", "1.0.0", "MIT OR Apache-2.0")
        [e] = self.build([dep], {"app 1.0.0": [("dep 1.0.0", None)]})
        self.assertEqual([f["id"] for f in e["fallback"]], ["MIT", "Apache-2.0"])
        self.assertIn("Permission is hereby granted", e["fallback"][0]["text"])

    def test_build_only_and_dev_only_crates_are_excluded(self):
        pkgs = [package(self.root, "runtime", "1.0.0", "MIT"), package(self.root, "builder", "1.0.0", "MIT"),
                package(self.root, "tester", "1.0.0", "MIT"), package(self.root, "deep", "1.0.0", "MIT")]
        edges = {"app 1.0.0": [("runtime 1.0.0", None), ("builder 1.0.0", "build"), ("tester 1.0.0", "dev")],
                 "builder 1.0.0": [("deep 1.0.0", None)]}
        self.assertEqual([e["name"] for e in self.build(pkgs, edges)], ["runtime"])

    def test_proc_macro_crates_are_kept_as_normal_dependencies(self):
        macro = package(self.root, "derive", "1.0.0", "MIT")
        macro["targets"] = [{"kind": ["proc-macro"]}]
        self.assertEqual([e["name"] for e in self.build([macro], {"app 1.0.0": [("derive 1.0.0", None)]})], ["derive"])

    def test_image_check_compares_the_three_lists(self):
        a, b, c = ("a", "1"), ("b", "1"), ("c", "1")
        self.assertEqual(notices.compare_sets({a, b}, {a, b}, {a}, {a, b}), [])
        self.assertEqual(notices.compare_sets({a, b}, {a, b}, {a}, {a, b, c}), [])
        self.assertTrue(any("c 1 is in the binary but has no notice" in p
                            for p in notices.compare_sets({a}, {a}, {a, c}, {a, c})))
        self.assertTrue(any("b 1 has a notice but is not in the binary" in p
                            for p in notices.compare_sets({a, b}, {a, b}, {a}, {a})))
        self.assertTrue(any("disagree on b 1" in p for p in notices.compare_sets({a, b}, {a}, {a}, {a, b})))

    def test_transitive_normal_dependencies_are_included(self):
        pkgs = [package(self.root, "a", "1.0.0", "MIT"), package(self.root, "b", "2.0.0", "MIT")]
        edges = {"app 1.0.0": [("a 1.0.0", None)], "a 1.0.0": [("b 2.0.0", None)]}
        self.assertEqual([(e["name"], e["version"]) for e in self.build(pkgs, edges)], [("a", "1.0.0"), ("b", "2.0.0")])

    def test_crate_with_nothing_fails_and_names_it(self):
        dep = package(self.root, "bare", "0.3.0", None)
        with self.assertRaises(notices.NoticesError) as cm:
            self.build([dep], {"app 1.0.0": [("bare 0.3.0", None)]})
        self.assertIn("bare@0.3.0", str(cm.exception))

    def test_reviewed_exception_is_allowed_with_a_note(self):
        dep = package(self.root, "bare", "0.3.0", None)
        [e] = self.build([dep], {"app 1.0.0": [("bare 0.3.0", None)]}, {"bare@0.3.0": "reviewed: public domain"})
        self.assertEqual(e["note"], "reviewed: public domain")

    def test_unknown_license_without_canonical_text_fails(self):
        dep = package(self.root, "odd", "1.0.0", "Odd-1.0")
        with self.assertRaises(notices.NoticesError):
            self.build([dep], {"app 1.0.0": [("odd 1.0.0", None)]})

    def test_license_expression_parsing(self):
        self.assertEqual(notices.license_ids("MIT/Apache-2.0"), ["MIT", "Apache-2.0"])
        self.assertEqual(notices.license_ids("(Apache-2.0 WITH LLVM-exception) AND MIT"),
                         ["Apache-2.0", "LLVM-exception", "MIT"])

    def test_every_allowlisted_license_has_a_canonical_text(self):
        import re
        allow = re.search(r"allow = \[(.*?)\]", open(os.path.join(notices.HERE, "..", "deny.toml")).read(), re.S).group(1)
        for lid in sorted({i for s in re.findall(r'"([^"]+)"', allow) for i in notices.license_ids(s)}):
            self.assertTrue(os.path.isfile(os.path.join(notices.SPDX_DIR, lid + ".txt")), lid)

    def test_markdown_lists_the_same_crates_as_the_entries(self):
        dep = package(self.root, "dep", "1.0.0", "MIT", {"LICENSE": "text"})
        entries = self.build([dep], {"app 1.0.0": [("dep 1.0.0", None)]}) + [notices.musl_entry("1.2.5-r1")]
        self.assertEqual(notices.markdown_crates(notices.render_markdown(entries)), {("dep", "1.0.0"), ("musl", "1.2.5-r1")})


if __name__ == "__main__":
    unittest.main()
