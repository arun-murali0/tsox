#!/usr/bin/env python3
"""Run tsc as an oracle and optionally record TSOX output for each fixture."""
import json
import os
import shlex
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent
MANIFEST = json.loads((ROOT / "manifest.json").read_text(encoding="utf-8"))
TS_CMD = shlex.split(os.environ.get(
    "TSC_CMD",
    "tsc --noEmit --strict --target ES2022 --lib ES2022,DOM",
))
TSOX_TEMPLATE = os.environ.get("TSOX_CMD")


def run(command):
    try:
        proc = subprocess.run(
            command, cwd=ROOT, text=True, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, timeout=90, check=False,
        )
        return {"exit_code": proc.returncode, "output": proc.stdout}
    except subprocess.TimeoutExpired as exc:
        out = exc.stdout or ""
        if isinstance(out, bytes):
            out = out.decode(errors="replace")
        return {"exit_code": 124, "output": out + "\nTIMEOUT after 90 seconds"}
    except FileNotFoundError as exc:
        return {"exit_code": 127, "output": str(exc)}


report = []
for item in MANIFEST:
    path = ROOT / item["file"]
    ts = run(TS_CMD + [str(path)])
    actual = "pass" if ts["exit_code"] == 0 else "fail"
    row = {
        "file": item["file"],
        "expected_tsc": item["expected_tsc"],
        "tsc_exit_code": ts["exit_code"],
        "tsc_match": actual == item["expected_tsc"],
        "tsc_output": ts["output"],
    }
    if TSOX_TEMPLATE:
        command = [part.replace("{file}", str(path)) for part in shlex.split(TSOX_TEMPLATE)]
        result = run(command)
        row["tsox_command"] = command
        row["tsox_exit_code"] = result["exit_code"]
        row["tsox_output"] = result["output"]
    report.append(row)

(ROOT / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
matched = sum(row["tsc_match"] for row in report)
print(f"TypeScript oracle: {matched}/{len(report)} fixture outcomes match the manifest.")
if TSOX_TEMPLATE:
    print(f"TSOX output captured for {sum('tsox_exit_code' in row for row in report)} fixtures.")
print(f"Full output: {ROOT / 'report.json'}")
if matched != len(report):
    raise SystemExit(1)
