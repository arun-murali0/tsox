#!/usr/bin/env python3
"""Fail when a workspace crate depends on a crate at its own layer or above.

The rule (docs/architecture/workspace.md): a crate may depend only on crates with a lower rank.
Dev-dependencies count too, so a test cannot quietly pull a layer upwards. Run from anywhere:
`python3 scripts/check-layering.py`.
"""
import json
import subprocess
import sys

# Lowest first. A crate may depend on any crate in an earlier layer, never on one in its own
# layer or a later one. The front ends share the top layer, so none depends on another.
LAYERS = [
    ["tsox-base"],
    ["tsox-types"],
    ["tsox-checker"],
    ["tsox-project"],
    ["tsox-db"],
    ["tsox"],
    ["tsox-cli", "tsox-wasm", "tsox-xtask"],
]
RANK = {name: rank for rank, layer in enumerate(LAYERS) for name in layer}


def main() -> int:
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps", "--locked"],
        check=True, capture_output=True, text=True,
    ).stdout
    packages = json.loads(out)["packages"]
    names = {p["name"] for p in packages}
    problems = []

    unknown = sorted(names - set(RANK))
    if unknown:
        problems.append(
            f"crate(s) {', '.join(unknown)} have no layer; add them to LAYERS in "
            "scripts/check-layering.py and to docs/architecture/workspace.md"
        )
    missing = sorted(set(RANK) - names)
    if missing:
        problems.append(f"layer(s) {', '.join(missing)} listed here but not in the workspace")

    for pkg in packages:
        mine = RANK.get(pkg["name"])
        if mine is None:
            continue
        for dep in pkg["dependencies"]:
            theirs = RANK.get(dep["name"])
            if theirs is None:
                continue  # an external crate
            if theirs >= mine:
                kind = dep["kind"] or "normal"
                problems.append(
                    f"{pkg['name']} ({kind}) depends on {dep['name']}, which is not below it"
                )

    for problem in problems:
        print(f"layering: {problem}", file=sys.stderr)
    if not problems:
        print("layering: ok (" + " < ".join(" | ".join(layer) for layer in LAYERS) + ")")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
