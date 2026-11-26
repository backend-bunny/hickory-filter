import gzip, io, json, os, sys, tarfile, tempfile, unittest

sys.path.insert(0, os.path.dirname(__file__))
import lockfile

CONFIG = """contents:
  repositories:
    - https://packages.wolfi.dev/os
    - '@local ./packages'
  packages:
    - ca-certificates-bundle
    - hickory-filter@local
entrypoint:
  command: /usr/bin/hickory-filter
"""


def lock_for(config, extra=()):
    pk = [{"name": n, "url": f"https://packages.wolfi.dev/os/x86_64/{n}.apk"} for n in lockfile.wolfi_packages(config)]
    pk += [{"name": n, "url": f"https://packages.wolfi.dev/os/x86_64/{n}.apk"} for n in ("wolfi-baselayout", *extra)]
    return {"version": "v1", "config": {"name": "apko.yaml", "checksum": lockfile.config_checksum(lockfile.strip(config))},
            "contents": {"repositories": [{"name": "w", "url": "u", "architecture": "x86_64"}], "packages": pk}}


def tar_gz(files):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:") as t:
        for name, data in files.items():
            i = tarfile.TarInfo(name); i.size = len(data); t.addfile(i, io.BytesIO(data))
    return gzip.compress(buf.getvalue())


class Lockfile(unittest.TestCase):
    def test_unchanged_lock_passes(self):
        self.assertEqual(lockfile.problems(CONFIG, lock_for(CONFIG)), [])

    def test_local_entries_are_not_part_of_the_locked_set(self):
        self.assertEqual(lockfile.wolfi_packages(CONFIG), ["ca-certificates-bundle"])
        self.assertNotIn("@local", lockfile.strip(CONFIG))

    def test_added_package_without_relock_names_the_package(self):
        changed = CONFIG.replace("    - ca-certificates-bundle\n", "    - ca-certificates-bundle\n    - tzdata\n")
        errs = lockfile.problems(changed, lock_for(CONFIG))
        self.assertTrue(any("tzdata" in e and "not in apko.lock.json" in e for e in errs), errs)

    def test_dropped_package_is_a_config_mismatch(self):
        changed = CONFIG.replace("    - ca-certificates-bundle\n", "")
        errs = lockfile.problems(changed, lock_for(CONFIG))
        self.assertTrue(any("different apko.yaml" in e for e in errs), errs)

    def test_any_other_config_change_is_a_mismatch(self):
        errs = lockfile.problems(CONFIG.replace("/usr/bin/hickory-filter", "/bin/x"), lock_for(CONFIG))
        self.assertTrue(any("different apko.yaml" in e for e in errs), errs)

    def test_local_change_alone_does_not_need_a_relock(self):
        changed = CONFIG.replace("hickory-filter@local", "other@local")
        self.assertEqual(lockfile.problems(changed, lock_for(CONFIG)), [])

    def test_lock_holding_a_local_package_is_rejected(self):
        lock = lock_for(CONFIG)
        lock["contents"]["packages"].append({"name": "hickory-filter", "url": "./packages/x86_64/hickory-filter.apk"})
        self.assertTrue(any("local package" in e for e in lockfile.problems(CONFIG, lock)))

    def test_merge_adds_the_local_apk_with_byte_ranges(self):
        sig = tar_gz({".SIGN.RSA.k.pub": b"sig"})
        ctl = tar_gz({".PKGINFO": b"pkgname = hickory-filter\npkgver = 0.1.0-r0\narch = x86_64\n"})
        data = tar_gz({"usr/bin/hickory-filter": b"bin"})
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(f"{d}/packages/x86_64")
            apk = f"{d}/packages/x86_64/hickory-filter-0.1.0-r0.apk"
            open(apk, "wb").write(sig + ctl + data)
            merged = lockfile.merge(CONFIG, lock_for(CONFIG), apk, apk, f"{d}/packages", "x86_64")
        local = [p for p in merged["contents"]["packages"] if p["name"] == "hickory-filter"][0]
        self.assertEqual(local["version"], "0.1.0-r0")
        self.assertEqual(local["signature"]["range"], f"bytes=0-{len(sig) - 1}")
        self.assertEqual(local["control"]["range"], f"bytes={len(sig)}-{len(sig) + len(ctl) - 1}")
        self.assertEqual(local["data"]["range"], f"bytes={len(sig) + len(ctl)}-{len(sig) + len(ctl) + len(data) - 1}")
        self.assertTrue(local["checksum"].startswith("Q1"))
        self.assertEqual(merged["config"]["checksum"], lockfile.config_checksum(CONFIG))

    def test_merge_refuses_a_drifted_lock(self):
        with self.assertRaises(SystemExit):
            lockfile.merge(CONFIG.replace("ca-certificates-bundle", "tzdata"), lock_for(CONFIG), "x", "x", ".", "x86_64")


if __name__ == "__main__":
    unittest.main()
