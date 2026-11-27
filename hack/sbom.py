#!/usr/bin/env python3
"""Add the Rust crates of the binary to the image's SPDX SBOM (standard library only).

apko's SBOM lists the packages installed in the image (APKs). The crates compiled
into the static binary are not packages of the image, so they are added here from
the same crate list the notices are generated from, each as a package with a
`pkg:cargo` identifier, contained by the hickory-filter package.

  sbom.py --sbom out/sbom-x86_64.spdx.json --notices out/notices.json --output out/sbom.spdx.json
"""
import argparse
import json
import re
import sys
import urllib.parse

ROOT_PACKAGE = "hickory-filter"


def spdx_id(name: str, version: str) -> str:
    return "SPDXRef-Package-cargo-" + re.sub(r"[^A-Za-z0-9.-]", "-", f"{name}-{version}")


def spdx_license(expression: str) -> str:
    """A declared Cargo license expression as an SPDX expression ("/" meant OR)."""
    if not expression:
        return "NOASSERTION"
    return expression.replace("/", " OR ")


def merge(sbom: dict, notices: dict) -> dict:
    roots = [p for p in sbom["packages"] if p["name"] == ROOT_PACKAGE and p["SPDXID"].startswith("SPDXRef-Package-apk-")]
    if len(roots) != 1:
        raise SystemExit(f"expected one {ROOT_PACKAGE} package in the SBOM, found {len(roots)}")
    root = roots[0]["SPDXID"]
    out = json.loads(json.dumps(sbom))
    have = {p["SPDXID"] for p in out["packages"]}
    for c in notices["crates"]:
        if c["name"] == "musl":
            continue  # the C library is part of the build environment and is covered in the notices
        sid = spdx_id(c["name"], c["version"])
        if sid in have:
            continue
        have.add(sid)
        purl = f"pkg:cargo/{urllib.parse.quote(c['name'])}@{urllib.parse.quote(c['version'])}"
        out["packages"].append({
            "SPDXID": sid,
            "name": c["name"],
            "versionInfo": c["version"],
            "downloadLocation": "NOASSERTION",
            "filesAnalyzed": False,
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": spdx_license(c["license"]),
            "copyrightText": "NOASSERTION",
            "supplier": "NOASSERTION",
            "externalRefs": [{"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl", "referenceLocator": purl}],
        })
        out["relationships"].append({"spdxElementId": root, "relationshipType": "CONTAINS", "relatedSpdxElement": sid})
    return out


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--sbom", required=True)
    ap.add_argument("--notices", required=True)
    ap.add_argument("--output", required=True)
    a = ap.parse_args(argv[1:])
    merged = merge(json.load(open(a.sbom)), json.load(open(a.notices)))
    with open(a.output, "w") as f:
        json.dump(merged, f, indent=1)
        f.write("\n")
    print(f"sbom: {len(merged['packages'])} packages")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
