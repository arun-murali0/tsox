import json, re, sys, collections
path = sys.argv[1]
only = set(c for c in (sys.argv[2] if len(sys.argv) > 2 else "").split(",") if c)
d = json.load(open(path))
code_of = lambda s: re.match(r"TS(\d+)", s).group(1) if re.match(r"TS(\d+)", s) else "?"
agree = collections.Counter(); differ = collections.Counter()
bad = []
for f in d["files"]:
    src = None
    for l in f["lines"]:
        tsox = collections.Counter(code_of(x) for x in l["tsox"])
        tsc = collections.Counter(code_of(x) for x in l["tsc"])
        for code in set(tsox) | set(tsc):
            if only and code not in only:
                continue
            if tsox[code] == tsc[code]:
                agree[code] += 1
            else:
                differ[code] += 1
                bad.append((f["file"], l["line"], code, tsox[code], tsc[code]))
codes = sorted(set(agree) | set(differ))
print(f"{'code':6} {'agree':>7} {'differ':>7}")
for c in codes:
    print(f"TS{c:4} {agree[c]:7} {differ[c]:7}")
print(f"\nfiles compared: {len(d['files'])}   differing lines: {len(bad)}")
shown = 0
for file, line, code, a, b in bad[:8]:
    kind = "FALSE POSITIVE (tsox only)" if a > b else "GAP (tsc only)"
    lines = open(file).read().split("\n")
    lo, hi = max(0, line - 6), min(len(lines), line + 2)
    print(f"\n--- {file}:{line} TS{code} {kind}")
    for i in range(lo, hi):
        print(f"{'>>' if i + 1 == line else '  '} {i+1:3} {lines[i]}")
sys.exit(1 if bad else 0)
