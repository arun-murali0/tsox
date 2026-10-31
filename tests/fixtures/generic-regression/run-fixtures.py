#!/usr/bin/env python3
"""Run TypeScript oracle and optional tsox checks for every fixture."""
import json
import os
import shlex
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent
MANIFEST = json.loads((ROOT / "manifest.json").read_text())
TS_CMD = shlex.split(os.environ.get(
    "TSC_CMD",
    "npx --yes --package typescript tsc --noEmit --strict --target ES2022 --lib ES2022,DOM"
))
TSOX_TEMPLATE = os.environ.get("TSOX_CMD")
report = []

def run(command):
    try:
        p = subprocess.run(command, cwd=ROOT, text=True, stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, timeout=90)
        return {"exit_code": p.returncode, "output": p.stdout}
    except subprocess.TimeoutExpired as e:
        out = e.stdout or ""
        if isinstance(out, bytes):
            out = out.decode(errors="replace")
        return {"exit_code": 124, "output": out + "\nTIMEOUT after 90 seconds"}
    except FileNotFoundError as e:
        return {"exit_code": 127, "output": str(e)}

for item in MANIFEST:
    path = ROOT / item["file"]
    ts = run(TS_CMD + [str(path)])
    expected = item["expected_tsc"]
    actual = "pass" if ts["exit_code"] == 0 else "fail"
    row = {
        "file": item["file"],
        "expected_tsc": expected,
        "tsc_exit_code": ts["exit_code"],
        "tsc_match": actual == expected,
        "tsc_output": ts["output"],
    }
    if TSOX_TEMPLATE:
        cmd = [part.replace("{file}", str(path)) for part in shlex.split(TSOX_TEMPLATE)]
        tsox = run(cmd)
        row["tsox_command"] = cmd
        row["tsox_exit_code"] = tsox["exit_code"]
        row["tsox_output"] = tsox["output"]
    report.append(row)

out = ROOT / "report.json"
out.write_text(json.dumps(report, indent=2) + "\n")
matched = sum(r["tsc_match"] for r in report)
print(f"TypeScript oracle: {matched}/{len(report)} fixtures match expected pass/fail.")
if TSOX_TEMPLATE:
    print(f"TSOX outputs captured for {sum('tsox_exit_code' in r for r in report)} fixtures.")
print(f"Full output: {out}")
if matched != len(report):
    raise SystemExit(1)
