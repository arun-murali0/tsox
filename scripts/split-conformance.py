#!/usr/bin/env python3
"""Split TypeScript conformance sources the way tsc's test runner does.

A source with `// @filename:` sections becomes a directory of those files; a source with
none is copied unchanged. Header directives before the first @filename belong to no file."""
import os, re, sys, shutil

FN = re.compile(r'^\s*//\s*@filename\s*:\s*(.+?)\s*$', re.I)

def norm(p):
    p = p.replace('\\', '/').lstrip('/')
    parts = [s for s in p.split('/') if s not in ('', '.', '..')]
    return '/'.join(parts)

def split(src, dst):
    os.makedirs(dst, exist_ok=True)
    n = 0
    for name in sorted(os.listdir(src)):
        path = os.path.join(src, name)
        if not name.endswith(('.ts', '.tsx')) or not os.path.isfile(path):
            continue
        text = open(path, encoding='utf-8', errors='replace').read()
        lines = text.split('\n')
        starts = [i for i, l in enumerate(lines) if FN.match(l)]
        if not starts:
            shutil.copy(path, os.path.join(dst, name))
            continue
        base = os.path.join(dst, name[:-3] if name.endswith('.ts') else name[:-4])
        used = set()
        for k, i in enumerate(starts):
            end = starts[k + 1] if k + 1 < len(starts) else len(lines)
            rel = norm(FN.match(lines[i]).group(1))
            if not rel or rel in used:
                continue
            used.add(rel)
            out = os.path.join(base, rel)
            os.makedirs(os.path.dirname(out), exist_ok=True)
            open(out, 'w', encoding='utf-8').write('\n'.join(lines[i + 1:end]) + '\n')
        n += 1
    return n

if __name__ == '__main__':
    src_root, dst_root = sys.argv[1], sys.argv[2]
    for part in sorted(os.listdir(src_root)):
        d = os.path.join(dst_root, part)
        if os.path.isdir(d): shutil.rmtree(d)
        print(part, split(os.path.join(src_root, part), d))
