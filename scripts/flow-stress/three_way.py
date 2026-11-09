"""Joins tsc's type for each statement-level read with what the forward overlay and the backward
flow walk gave it (from `TSOX_FLOW_SHADOW=all`), and says which of the two agrees with tsc."""
import re, sys, collections, subprocess, json
d, out, show = sys.argv[1], sys.argv[2], int(sys.argv[3])
tsc = {}
for line in subprocess.run(["node", "scripts/flow-stress/types.js", d], capture_output=True, text=True).stdout.splitlines():
    m = re.match(r"(\S+):(\d+):(\d+) (.*)", line)
    if m:
        tsc[(m.group(1), int(m.group(2)), int(m.group(3)))] = m.group(4)
norm = lambda s: " | ".join(sorted(x.strip() for x in s.split("|")))
both = {}
for l in open(out):
    m = re.match(r"(\S+):(\d+):(\d+): warning: TSR9201 flow shadow: '(\w+)' is '(.*)' by the forward pass and '(.*)' by the flow walk", l)
    if m:
        f, line, col, name, o, fl = m.groups(); both[(f, int(line), int(col))] = (name, norm(o), norm(fl)); continue
    m = re.match(r"(\S+):(\d+):(\d+): warning: TSR9202 flow shadow: '(\w+)' is '(.*)' by both", l)
    if m:
        f, line, col, name, t = m.groups(); both[(f, int(line), int(col))] = (name, norm(t), norm(t))
cat = collections.Counter(); examples = collections.defaultdict(list)
for key, (name, o, fl) in both.items():
    if key not in tsc:
        continue
    t = norm(tsc[key])
    kind = ("both match tsc" if o == t and fl == t else
            "overlay matches tsc, flow does not" if o == t else
            "flow matches tsc, overlay does not" if fl == t else
            "neither matches tsc" if o != fl else "both agree, tsc differs")
    cat[kind] += 1
    if len(examples[kind]) < show:
        examples[kind].append((key, name, o, fl, t))
total = sum(cat.values())
print(f"statement-level reads compared with tsc: {total}")
for kind, n in cat.most_common():
    print(f"  {n:6}  {kind}")
for kind in ("overlay matches tsc, flow does not", "flow matches tsc, overlay does not", "neither matches tsc"):
    for (f, line, col), name, o, fl, t in examples[kind]:
        src = open(f).read().split("\n")
        print(f"\n--- {kind}: {f.split('/')[-1]}:{line}:{col} {name}: overlay {o!r} flow {fl!r} tsc {t!r}")
        for i in range(max(0, line - 5), line):
            print(f"{'>>' if i + 1 == line else '  '} {i+1:3} {src[i]}")
