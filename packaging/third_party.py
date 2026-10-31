#!/usr/bin/env python3
"""Collect the license and notice files of everything built into Kyte.

    packaging/third_party.py <out-dir>

Writes <out-dir>/README.md, an index of the third-party crates built into the
release and the files kept under vendor/, with each one's license and a link to
its source, and next to it one file per notice, named <name>-<version>-<FILE>,
so the directory is flat and a package's file list can take it whole. The
tarball, the RPM and the .deb ship it as third-party/. Run from the repository
root; packaging/dist.sh runs it for you.
"""
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

NOTICE = ("license", "licence", "copying", "copyright", "notice", "unlicense", "acknowledg")
TARGET = "x86_64-unknown-linux-musl"

# Kept in vendor/ rather than built from a crate. See the READMEs there.
VENDORED = [
    (
        "tree-sitter-latex scanner",
        "vendor/tree-sitter-latex/LICENSE",
        "MIT",
        "https://github.com/latex-lsp/tree-sitter-latex",
    ),
    (
        "tree-sitter-bibtex parser",
        "vendor/tree-sitter-bibtex/LICENSE",
        "MIT",
        "https://github.com/latex-lsp/tree-sitter-bibtex",
    ),
    (
        "en_US dictionary (SCOWL)",
        "vendor/dictionaries/en_US/LICENSE",
        "SCOWL's terms, in its notice",
        "https://github.com/wooorm/dictionaries",
    ),
]


def built_in(meta: dict) -> list:
    """The packages that reach the binary: not the ones only the tests use."""
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]
    seen, stack = set(), [root]
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            if any(k["kind"] in (None, "build") for k in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    seen.discard(root)
    return [p for p in meta["packages"] if p["id"] in seen and p["source"]]


def main(out: Path) -> None:
    cargo = os.environ.get("CARGO", "cargo")
    command = [cargo, "metadata", "--locked", "--format-version", "1", "--filter-platform", TARGET]
    meta = json.loads(subprocess.check_output(command, text=True))
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    lines = [
        "# Third-party notices",
        "",
        "What's built into this release of Kyte, with its declared license. Each notice file",
        "here is copied unchanged; the link is the source as published.",
        "",
    ]
    for title, notice, license_name, source in VENDORED:
        flat = title.replace(" ", "-").replace("(", "").replace(")", "") + "-LICENSE"
        shutil.copyfile(notice, out / flat)
        lines += [
            f"## {title}",
            "",
            f"License: {license_name}. [Source]({source}).",
            f"Notices: [{flat}]({flat})",
            "",
        ]
    crates = sorted(built_in(meta), key=lambda p: (p["name"], p["version"]))
    for pkg in crates:
        name, version = pkg["name"], pkg["version"]
        root = Path(pkg["manifest_path"]).parent
        copied = []
        for entry in sorted(root.iterdir()):
            if entry.is_symlink() or not entry.name.lower().startswith(NOTICE):
                continue
            files = [entry] if entry.is_file() else sorted(
                f for f in entry.rglob("*") if f.is_file() and not f.is_symlink()
            )
            for f in files:
                flat = f"{name}-{version}-" + "-".join(f.relative_to(root).parts)
                shutil.copyfile(f, out / flat)
                copied.append(flat)
        lines += [
            f"## {name} {version}",
            "",
            f"License: `{pkg.get('license') or 'see the source'}`. "
            f"[Source](https://crates.io/crates/{name}/{version}).",
            ("Notices: " + ", ".join(f"[{c}]({c})" for c in copied))
            if copied
            else "Notices: none in the crate; see its source.",
            "",
        ]
    (out / "README.md").write_text("\n".join(lines) + "\n")
    print(f"third-party: {len(crates)} crates, {sum(1 for _ in out.iterdir()) - 1} notice files in {out}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(Path(sys.argv[1]))
