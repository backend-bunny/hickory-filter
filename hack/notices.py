#!/usr/bin/env python3
"""Third-party license notices for the statically linked binary (standard library only).

  notices.py generate --source DIR --out-dir DIR --target TRIPLE --musl-version V
      Reads `cargo metadata` for the vendored source tree in DIR, walks the normal
      dependencies of its root package on TRIPLE, and writes notices.json and
      THIRD-PARTY-NOTICES.md into --out-dir.
  notices.py check-image THIRD-PARTY-NOTICES.md notices.json BINARY
      Fails unless the crate sets of the Markdown, the JSON and the crate list
      embedded in the binary by cargo-auditable are the same.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
SPDX_DIR = os.path.join(HERE, "spdx-licenses")
EXCEPTIONS = os.path.join(HERE, "notices-exceptions.json")
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|copyright|unlicense)", re.IGNORECASE)
SCHEMA = "hickory-filter/notices/v1"
MUSL_COPYRIGHT = "Copyright © 2005-2020 Rich Felker, et al."


class NoticesError(Exception):
    pass


def license_ids(expression: str) -> list[str]:
    """SPDX identifiers (and exception identifiers) named by an expression."""
    ids = []
    for tok in re.findall(r"[A-Za-z0-9.+-]+", expression.replace("/", " OR ")):
        if tok.upper() in ("AND", "OR", "WITH"):
            continue
        tok = tok.rstrip("+") if tok.endswith("+") else tok
        if tok not in ids:
            ids.append(tok)
    return ids


def read_text(path: str) -> str:
    with open(path, "rb") as f:
        return f.read().decode("utf-8", errors="replace")


def crate_files(directory: str) -> list[dict]:
    out = []
    for name in sorted(os.listdir(directory)):
        path = os.path.join(directory, name)
        if os.path.isfile(path) and LICENSE_FILE.match(name):
            out.append({"name": name, "text": read_text(path)})
    return out


def fallback_texts(expression: str, spdx_dir: str) -> list[dict]:
    out = []
    for lid in license_ids(expression):
        path = os.path.join(spdx_dir, lid + ".txt")
        if not os.path.isfile(path):
            raise NoticesError(f"no canonical text for license {lid} (add it to hack/spdx-licenses/)")
        out.append({"id": lid, "text": read_text(path)})
    return out


def normal_dependency_ids(metadata: dict) -> list[str]:
    """Package ids reachable from the root through normal (not build or dev) dependencies.

    Procedural macros are normal dependencies in cargo's terms, so they stay in:
    a notices file that names a few crates more than the linked code is the safe
    side of the obligation."""
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    root = metadata["resolve"]["root"]
    seen, stack = set(), [root]
    while stack:
        cur = stack.pop()
        for dep in nodes[cur]["deps"]:
            if any(k.get("kind") is None for k in dep.get("dep_kinds", [])) and dep["pkg"] not in seen:
                seen.add(dep["pkg"])
                stack.append(dep["pkg"])
    return sorted(seen)


def build_entries(metadata: dict, spdx_dir: str = SPDX_DIR, exceptions: dict | None = None) -> list[dict]:
    exceptions = exceptions or {}
    packages = {p["id"]: p for p in metadata["packages"]}
    entries, failures = [], []
    for pid in normal_dependency_ids(metadata):
        p = packages[pid]
        key = f"{p['name']}@{p['version']}"
        files = crate_files(os.path.dirname(p["manifest_path"]))
        declared = p.get("license") or ""
        entry = {"name": p["name"], "version": p["version"], "source": p.get("source") or "path",
                 "license": declared, "files": files}
        if not files:
            if declared:
                entry["fallback"] = fallback_texts(declared, spdx_dir)
            elif key in exceptions:
                entry["note"] = exceptions[key]
            else:
                failures.append(f"{key} has no license file and declares no license")
        entries.append(entry)
    if failures:
        raise NoticesError("\n".join(failures))
    return sorted(entries, key=lambda e: (e["name"], e["version"]))


def musl_entry(version: str, spdx_dir: str = SPDX_DIR) -> dict:
    return {"name": "musl", "version": version, "source": "Alpine Linux package musl (static libc)",
            "license": "MIT", "files": [], "note": MUSL_COPYRIGHT, "fallback": fallback_texts("MIT", spdx_dir)}


def render_markdown(entries: list[dict]) -> str:
    out = ["# Third-party notices", "",
           "hickory-filter is statically linked. It contains the components below, "
           "each under the license terms shown.", ""]
    for e in entries:
        out += [f"## {e['name']} {e['version']}", "", f"- Source: {e['source']}", f"- License: {e['license'] or 'not declared'}"]
        if e.get("note"):
            out.append(f"- Note: {e['note']}")
        out.append("")
        for f in e["files"]:
            out += [f"### {f['name']}", "", "~~~~", f["text"].rstrip("\n"), "~~~~", ""]
        for f in e.get("fallback", []):
            out += [f"### {f['id']} (canonical text)", "", "~~~~", f["text"].rstrip("\n"), "~~~~", ""]
    return "\n".join(out)


def markdown_crates(text: str) -> set[tuple[str, str]]:
    return {tuple(m.groups()) for m in re.finditer(r"^## (\S+) (\S+)$", text, re.MULTILINE)}


def auditable_crates(binary: str) -> tuple[set[tuple[str, str]], set[tuple[str, str]]]:
    """(runtime crates, all crates) from the list cargo-auditable embeds in the binary."""
    with tempfile.TemporaryDirectory() as d:
        section = os.path.join(d, "dep-v0")
        subprocess.run(["objcopy", f"--dump-section=.dep-v0={section}", binary, os.path.join(d, "unused")], check=True)
        raw = open(section, "rb").read()
    if not raw:
        raise NoticesError(f"{binary} has no cargo-auditable data (.dep-v0)")
    data = json.loads(zlib.decompress(raw))
    pkgs = [p for p in data["packages"] if not p.get("root")]
    runtime = {(p["name"], p["version"]) for p in pkgs if p.get("kind", "runtime") == "runtime"}
    return runtime, {(p["name"], p["version"]) for p in pkgs}


def compare_sets(md: set, js: set, runtime: set, everything: set) -> list[str]:
    """The notices file and notices.json list the same crates. The binary's embedded
    list is the scanner's view: every crate it lists as runtime needs a notice, and
    the notices name no crate the binary does not list (cargo-auditable marks some
    procedural macros as build-only, which the notices keep as a safe superset)."""
    problems = []
    for c in sorted(md ^ js):
        problems.append(f"notices file and notices.json disagree on {c[0]} {c[1]}")
    for c in sorted(runtime - js):
        problems.append(f"{c[0]} {c[1]} is in the binary but has no notice")
    for c in sorted(js - everything):
        problems.append(f"{c[0]} {c[1]} has a notice but is not in the binary")
    return problems


def check_image(markdown: str, json_file: str, binary: str, extra: tuple = ("musl",)) -> list[str]:
    md = {c for c in markdown_crates(read_text(markdown)) if c[0] not in extra}
    js = {(e["name"], e["version"]) for e in json.load(open(json_file))["crates"] if e["name"] not in extra}
    runtime, everything = auditable_crates(binary)
    return compare_sets(md, js, runtime, everything)


def generate(source: str, out_dir: str, target: str, musl_version: str) -> int:
    proc = subprocess.run(["cargo", "metadata", "--locked", "--offline", "--format-version", "1",
                           "--filter-platform", target], cwd=source, capture_output=True, text=True)
    if proc.returncode:
        raise NoticesError("cargo metadata failed:\n" + proc.stderr)
    exceptions = json.load(open(EXCEPTIONS))
    entries = build_entries(json.loads(proc.stdout), SPDX_DIR, exceptions)
    entries.append(musl_entry(musl_version))
    entries.sort(key=lambda e: (e["name"], e["version"]))
    os.makedirs(out_dir, exist_ok=True)
    with open(os.path.join(out_dir, "notices.json"), "w") as f:
        json.dump({"schema": SCHEMA, "crates": entries}, f, indent=1, sort_keys=True)
        f.write("\n")
    with open(os.path.join(out_dir, "THIRD-PARTY-NOTICES.md"), "w") as f:
        f.write(render_markdown(entries) + "\n")
    print(f"notices: {len(entries)} components")
    return 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    g = sub.add_parser("generate")
    g.add_argument("--source", required=True); g.add_argument("--out-dir", required=True)
    g.add_argument("--target", default="x86_64-unknown-linux-musl"); g.add_argument("--musl-version", required=True)
    c = sub.add_parser("check-image")
    c.add_argument("markdown"); c.add_argument("json"); c.add_argument("binary")
    a = ap.parse_args(argv[1:])
    try:
        if a.cmd == "generate":
            return generate(a.source, a.out_dir, a.target, a.musl_version)
        problems = check_image(a.markdown, a.json, a.binary)
        for p in problems:
            print("error:", p, file=sys.stderr)
        return 1 if problems else 0
    except NoticesError as e:
        print("error:", e, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
