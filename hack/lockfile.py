#!/usr/bin/env python3
"""apko lockfile handling for hickory-filter (standard library only).

The committed apko.lock.json pins the Wolfi packages. The hickory-filter APK
is built fresh on every run, so it is not in the committed lock: `merge` adds
it at build time. `check` fails when apko.yaml and the lock disagree.

  lockfile.py strip   apko.yaml            print the config without @local entries
  lockfile.py check   apko.yaml lock.json  exit 1 and name the mismatch
  lockfile.py merge   apko.yaml lock.json package.apk out.json
"""
import base64
import hashlib
import json
import re
import sys
import zlib

LOCAL_LINE = re.compile(r"^\s*-\s*['\"]?(@local\b|[A-Za-z0-9._+-]+@local\b)")
PACKAGE_LINE = re.compile(r"^\s*-\s*([A-Za-z0-9._+-]+)(?:@([a-z]+))?(?:[=<>~].*)?\s*$")


def strip(text: str) -> str:
    """The configuration without the `@local` repository and packages."""
    return "".join(l for l in text.splitlines(keepends=True) if not LOCAL_LINE.match(l))


def config_checksum(text: str) -> str:
    return "sha256-" + base64.b64encode(hashlib.sha256(text.encode()).digest()).decode()


def wolfi_packages(text: str) -> list[str]:
    """Names listed under `contents.packages` that come from the pinned repository."""
    names, in_packages = [], False
    for line in strip(text).splitlines():
        if re.match(r"^\s{2}packages:\s*$", line):
            in_packages = True
        elif in_packages and PACKAGE_LINE.match(line):
            names.append(PACKAGE_LINE.match(line).group(1))
        elif in_packages and line.strip() and not line.startswith("    "):
            in_packages = False
    return names


def problems(config_text: str, lock: dict) -> list[str]:
    out = []
    locked = {p["name"] for p in lock.get("contents", {}).get("packages", [])}
    for name in wolfi_packages(config_text):
        if name not in locked:
            out.append(f"package {name} is in apko.yaml but not in apko.lock.json (run `task lock`)")
    if any(p.get("url", "").startswith("./") for p in lock.get("contents", {}).get("packages", [])):
        out.append("apko.lock.json contains a local package; the lock holds Wolfi packages only")
    expected = config_checksum(strip(config_text))
    got = lock.get("config", {}).get("checksum")
    if got != expected:
        out.append(f"apko.lock.json was generated from a different apko.yaml (checksum {got} != {expected}); run `task lock`")
    return out


def apk_segments(data: bytes) -> list[tuple[int, int]]:
    """Byte ranges (start, end inclusive) of the gzip members of an APK."""
    segments, pos = [], 0
    while pos < len(data):
        d = zlib.decompressobj(wbits=31)
        d.decompress(data[pos:])
        used = len(data) - pos - len(d.unused_data)
        segments.append((pos, pos + used - 1))
        pos += used
    return segments


def local_entry(apk_path: str, repository_url: str) -> dict:
    data = open(apk_path, "rb").read()
    segs = apk_segments(data)
    if len(segs) != 3:
        raise SystemExit(f"{apk_path}: expected signature, control and data segments, found {len(segs)}")
    (s0, s1), (c0, c1), (d0, d1) = segs
    sha1 = lambda b: "sha1-" + base64.b64encode(hashlib.sha1(b).digest()).decode()
    control = data[c0 : c1 + 1]
    import tarfile, io
    info = {}
    with tarfile.open(fileobj=io.BytesIO(zlib.decompress(control, 31)), mode="r:") as t:
        for line in t.extractfile(".PKGINFO").read().decode().splitlines():
            if " = " in line and not line.startswith("#"):
                k, v = line.split(" = ", 1)
                info.setdefault(k, v)
    return {
        "name": info["pkgname"],
        "url": repository_url,
        "version": info["pkgver"],
        "architecture": info["arch"],
        "signature": {"range": f"bytes={s0}-{s1}", "checksum": sha1(data[s0 : s1 + 1])},
        "control": {"range": f"bytes={c0}-{c1}", "checksum": sha1(control)},
        "data": {
            "range": f"bytes={d0}-{d1}",
            "checksum": "sha256-" + base64.b64encode(hashlib.sha256(data[d0 : d1 + 1]).digest()).decode(),
        },
        "checksum": "Q1" + base64.b64encode(hashlib.sha1(control).digest()).decode(),
    }


def merge(config_text: str, lock: dict, apk_path: str, apk_url: str, repo_dir: str, arch: str) -> dict:
    errs = problems(config_text, lock)
    if errs:
        raise SystemExit("\n".join(errs))
    merged = json.loads(json.dumps(lock))
    merged["config"] = {"name": "apko.yaml", "checksum": config_checksum(config_text)}
    merged["contents"]["repositories"].append(
        {"name": f"{repo_dir}/{arch}", "url": f"{repo_dir}/{arch}/APKINDEX.tar.gz", "architecture": arch}
    )
    merged["contents"]["packages"].append(local_entry(apk_path, apk_url))
    return merged


def main(argv: list[str]) -> int:
    if len(argv) >= 3 and argv[1] == "strip":
        sys.stdout.write(strip(open(argv[2]).read()))
        return 0
    if len(argv) == 4 and argv[1] == "check":
        errs = problems(open(argv[2]).read(), json.load(open(argv[3])))
        for e in errs:
            print("error:", e, file=sys.stderr)
        return 1 if errs else 0
    if len(argv) == 6 and argv[1] == "merge":
        text, lock = open(argv[2]).read(), json.load(open(argv[3]))
        apk = argv[4]
        arch = apk.rstrip("/").split("/")[-2]
        repo_dir = "/".join(apk.split("/")[:-2]) or "."
        merged = merge(text, lock, apk, apk, repo_dir, arch)
        json.dump(merged, open(argv[5], "w"), indent=2)
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
