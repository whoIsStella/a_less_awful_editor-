#!/usr/bin/env python3
"""Fail if either pure core has a direct or transitive GPUI dependency."""
import json
import subprocess

metadata = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--locked", "--format-version", "1"], text=True
))
packages = {package["id"]: package for package in metadata["packages"]}
nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
for core in ("ale-editor-core", "ale-systems-core"):
    root = next(key for key, package in packages.items() if package["name"] == core)
    pending = [(root, [core])]
    seen = set()
    while pending:
        package_id, path = pending.pop()
        if package_id in seen:
            continue
        seen.add(package_id)
        name = packages[package_id]["name"]
        if "gpui" in name.lower():
            raise SystemExit("Forbidden UI dependency: " + " -> ".join(path))
        for dependency in nodes[package_id]["dependencies"]:
            pending.append((dependency, path + [packages[dependency]["name"]]))
    print(f"PASS: {core} and {len(seen) - 1} transitive dependencies are GPUI-free")
