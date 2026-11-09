import re, sys, collections
d, out, show = sys.argv[1], sys.argv[2], int(sys.argv[3])
agreed = disagreed = unknown = unreachable = 0
for l in open(out):
    m = re.match(r"flow shadow: (\d+) agreed, (\d+) disagreed, (\d+) unknown, (\d+) unreachable", l)
    if m:
        a, b, c, e = map(int, m.groups()); agreed += a; disagreed += b; unknown += c; unreachable += e
diffs = []
for l in open(out):
    m = re.match(r"(\S+):(\d+):(\d+): warning: TSR9201 flow shadow: '(\w+)' is '(.*)' by the forward pass and '(.*)' by the flow walk", l)
    if m:
        diffs.append(m.groups())
print(f"reads answered by the walk: {agreed + disagreed}   agreed: {agreed}   disagreed: {disagreed}   (unknown: {unknown}, unreachable: {unreachable})")
shapes = collections.Counter()
for f, line, col, name, overlay, flow in diffs:
    shapes[(overlay, flow)] += 1
for (o, fl), n in shapes.most_common(8):
    print(f"{n:4}  forward {o!r:40} flow {fl!r}")
for f, line, col, name, overlay, flow in diffs[:show]:
    src = open(f).read().split("\n")
    lo = max(0, int(line) - 6)
    print(f"\n--- {f.split('/')[-1]}:{line}:{col} {name}: forward {overlay!r} flow {flow!r}")
    for i in range(lo, int(line)):
        print(f"{'>>' if i + 1 == int(line) else '  '} {i+1:3} {src[i]}")
sys.exit(1 if disagreed else 0)
