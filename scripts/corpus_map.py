#!/usr/bin/env python3
"""A per-file map of the tsc corpus, so a change is checked on the files it can affect.

  corpus_map.py build  RUN_DIR PREFIX MAP      # MAP from RUN_DIR/PREFIX-NN.json (compare-local --json, one per part)
  corpus_map.py select MAP OUT_DIR [--codes 2364,2357] [--differ] [--files-like TEXT]
  corpus_map.py diff   MAP RUN_JSON [RUN_JSON ...]   # verdicts of a partial run against the map

`select` copies the files into OUT_DIR/part-NN (a symlinked set runs empty in compare-local), keeping
every file whose tsc or tsox output has one of the codes (or, with --differ, every file that is not a
MATCH). Run scripts/compare-local.sh on OUT_DIR/part-NN for each part, then `diff`.
"""
import glob, json, os, re, shutil, sys


def codes(lines, side):
    out = set()
    for l in lines:
        for m in l[side]:
            out.add(m.split()[0].replace('TS', ''))
    return sorted(out)


def build(run_dir, prefix, map_path):
    table = {}
    for path in sorted(glob.glob(os.path.join(run_dir, f'{prefix}-[0-9][0-9].json'))):
        part = re.search(r'-(\d\d)\.json$', path).group(1)
        with open(path) as fh:
            data = json.load(fh)
        for f in data['files']:
            table[f['file']] = {
                'part': part,
                'verdict': f['verdict'],
                'tsc': codes(f['lines'], 'tsc'),
                'tsox': codes(f['lines'], 'tsox'),
            }
    with open(map_path, 'w') as fh:
        json.dump(table, fh, indent=0, sort_keys=True)
    print(f'{len(table)} files mapped ({sum(1 for v in table.values() if v["verdict"] == "MATCH")} match)')


def select(map_path, out_dir, wanted, differ, like):
    table = json.load(open(map_path))
    keep = []
    for path, row in table.items():
        hit = bool(wanted & (set(row['tsc']) | set(row['tsox'])))
        hit = hit or (differ and row['verdict'] != 'MATCH')
        hit = hit or (like and like in os.path.basename(path))
        if hit:
            keep.append((row['part'], path))
    shutil.rmtree(out_dir, ignore_errors=True)
    for part, path in keep:
        dest = os.path.join(out_dir, f'part-{part}')
        os.makedirs(dest, exist_ok=True)
        shutil.copy(path, dest)
    parts = sorted({p for p, _ in keep})
    print(f'{len(keep)} files in {len(parts)} parts: ' + ' '.join(f'part-{p}' for p in parts))


def diff(map_path, run_jsons):
    table = json.load(open(map_path))
    gained, lost, same = [], [], 0
    seen = set()
    for run in run_jsons:
        for f in json.load(open(run))['files']:
            base = os.path.basename(f['file'])
            key = next((p for p in table if os.path.basename(p) == base), None)
            if key is None:
                continue
            seen.add(key)
            before, after = table[key]['verdict'], f['verdict']
            if before == after:
                same += 1
            elif after == 'MATCH':
                gained.append((base, before))
            elif before == 'MATCH':
                lost.append((base, after))
    print(f'{len(seen)} files re-run: {len(gained)} now match, {len(lost)} stopped matching, {same} unchanged verdict')
    for base, before in gained[:20]:
        print('  + ', base, '(was', before + ')')
    for base, after in lost[:20]:
        print('  - ', base, '(now', after + ')')


if __name__ == '__main__':
    a = sys.argv[1:]
    if a[:1] == ['build']:
        build(a[1], a[2], a[3])
    elif a[:1] == ['select']:
        opts = a[3:]
        wanted = set()
        like = None
        if '--codes' in opts:
            wanted = set(opts[opts.index('--codes') + 1].split(','))
        if '--files-like' in opts:
            like = opts[opts.index('--files-like') + 1]
        select(a[1], a[2], wanted, '--differ' in opts, like)
    elif a[:1] == ['diff']:
        diff(a[1], a[2:])
    else:
        print(__doc__)
