#!/usr/bin/env python3
"""Compare tsox with tsc on small probe programs, and print what each one shows.

Each probe is a tiny project of one to three files. tsc is the reference: a probe MATCHES when
tsox reports the same error codes in the same files, is a GAP when tsox reports only some of
what tsc does (it stays silent or warns "not yet checked"), is a FALSE-POS when tsox reports
an error tsc does not, and is a DIFF when both report errors and the codes differ. The
"Valid programs" probes have no error in tsc, so any tsox error there is a false positive.

    python3 scripts/coverage-probes.py [--tsox PATH] [--tsc PATH] [--markdown] [--json FILE]

Needs a tsox binary (default: target/release/tsox, then target/debug/tsox) and tsc 6.0.3
(default: scripts/ts-diag-tool/node_modules/.bin/tsc after `npm ci` there, else `tsc` on PATH).
Only the standard library is used. docs/language-coverage.md is generated from this output.
"""
import argparse, json, os, re, shutil, subprocess, sys, tempfile
from collections import Counter, OrderedDict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CONFIG = ('{"compilerOptions":{"strict":true,"noEmit":true,"target":"es2022","module":"esnext",'
          '"moduleResolution":"bundler","lib":["es2022"],"types":[]},"include":["*.ts"]}')

P = []  # (area, name, {file: source})


def one(area, name, src):
    P.append((area, name, {'a.ts': src}))


def multi(area, name, **files):
    P.append((area, name, files))


def mod(name, a, b):
    multi('Modules', name, **{'a.ts': a, 'b.ts': b})


# --- basics
one('Basics','assign wrong primitive','export const x: number = "a";')
one('Basics','call with wrong argument','function f(a: number) {}\nf("x");\nexport {};')
one('Basics','wrong argument count','function f(a: number) {}\nf();\nexport {};')
one('Basics','implicit any parameter','function f(a) {}\nexport {};')
one('Basics','undefined name','export const y = notDeclared;')
one('Basics','null not assignable','export const x: string = null;')
one('Basics','possibly null access','function f(x: string | null) { return x.length; }\nexport {};')
one('Basics','use before assigned','function f() { let a: number; return a + 1; }\nexport {};')
one('Basics','return type mismatch','function f(): number { return "x"; }\nexport {};')
one('Basics','duplicate identifier','let a = 1;\nlet a = 2;\nexport {};')
# --- narrowing
one('Narrowing','typeof narrowing','function f(x: string | number) { if (typeof x === "string") { return x.toFixed(); } return x; }\nexport {};')
one('Narrowing','truthiness narrowing','function f(x: string | undefined) { if (x) { return x.length; } return x.length; }\nexport {};')
one('Narrowing','in operator narrowing','function f(x: {a: 1} | {b: 2}) { if ("a" in x) { return x.b; } return 0; }\nexport {};')
one('Narrowing','discriminated union','type S = {k: "a"; a: number} | {k: "b"; b: string};\nfunction f(s: S) { switch (s.k) { case "a": return s.b; default: return s.b; } }\nexport {};')
one('Narrowing','type predicate','function isS(x: unknown): x is string { return typeof x === "string"; }\nfunction g(x: unknown) { if (isS(x)) { x.toFixed(); } }\nexport {};')
one('Narrowing','instanceof narrowing','class A { a = 1 } class B { b = 2 }\nfunction f(x: A | B) { if (x instanceof A) { return x.b; } return x.b; }\nexport {};')
one('Narrowing','assertion function','function assertS(x: unknown): asserts x is string {}\nfunction g(x: unknown) { assertS(x); x.toFixed(); }\nexport {};')
one('Narrowing','valid narrowing (no errors)','function f(x: string | number | null) { if (x === null) return 0; if (typeof x === "number") return x + 1; return x.length; }\nexport {};')
# --- objects
one('Objects','excess property','const o: {a: number} = {a: 1, b: 2};\nexport {};')
one('Objects','missing property','const o: {a: number; b: string} = {a: 1};\nexport {};')
one('Objects','interface extends','interface A {a: number} interface B extends A {b: string}\nconst b: B = {a: 1};\nexport {};')
one('Objects','readonly assignment','const o: {readonly a: number} = {a: 1};\no.a = 2;\nexport {};')
one('Objects','index signature','const o: {[k: string]: number} = {a: "x"};\nexport {};')
one('Objects','optional property misuse','const o: {a?: number} = {};\nconst n: number = o.a;\nexport {};')
one('Objects','destructuring missing name','const {a, b} = {a: 1};\nexport {};')
one('Objects','spread into wrong type','const a = [1, 2];\nconst b: string[] = [...a];\nexport {};')
one('Objects','object spread valid (no errors)','const a = {x: 1}; const b = {...a, y: "s"}; const c: {x: number; y: string} = b;\nexport {};')
one('Objects','satisfies','const o = {a: 1} satisfies {a: string};\nexport {};')
one('Objects','as const readonly tuple','const t = [1, 2] as const;\nt.push(3);\nexport {};')
# --- functions and generics
one('Generics','generic inference wrong use','function id<T>(x: T): T { return x; }\nconst s: string = id(1);\nexport {};')
one('Generics','constraint violation','function f<T extends {a: number}>(x: T) {}\nf({b: 1});\nexport {};')
one('Generics','overload no match','function f(a: string): string;\nfunction f(a: number): number;\nfunction f(a: any) { return a; }\nf(true);\nexport {};')
one('Generics','function type mismatch','const f: (a: number) => string = (a) => a;\nexport {};')
one('Generics','generic class member','class Box<T> { v!: T }\nconst b = new Box<number>();\nconst s: string = b.v;\nexport {};')
one('Generics','array map callback','const a = [1, 2, 3];\nconst b: string[] = a.map(x => x * 2);\nexport {};')
one('Generics','Promise then chain','const p = Promise.resolve(1);\nconst q: Promise<string> = p.then(x => x + 1);\nexport {};')
one('Generics','default type parameter','interface B<T = string> {v: T}\nconst b: B = {v: 1};\nexport {};')
# --- classes
one('Classes','missing member','class A { x = 1 }\nnew A().y;\nexport {};')
one('Classes','abstract instantiation','abstract class A {}\nnew A();\nexport {};')
one('Classes','private access','class A { private p = 1 }\nnew A().p;\nexport {};')
one('Classes','hash private access','class A { #p = 1 }\nnew A().#p;\nexport {};')
one('Classes','implements missing','interface I {m(): void}\nclass C implements I {}\nexport {};')
one('Classes','override wrong type','class A { m(): number { return 1 } }\nclass B extends A { m(): string { return "" } }\nexport {};')
one('Classes','property not initialized','class A { x: number }\nexport {};')
one('Classes','parameter properties','class A { constructor(public x: number) {} }\nconst s: string = new A(1).x;\nexport {};')
one('Classes','static member','class A { static s = 1 }\nconst n: string = A.s;\nexport {};')
one('Classes','getter type','class A { get v(): number { return 1 } }\nconst s: string = new A().v;\nexport {};')
# --- enums, tuples, advanced types
one('Types','enum assignment','enum E {A, B}\nconst e: E = "x";\nexport {};')
one('Types','tuple length','const t: [number, string] = [1];\nexport {};')
one('Types','keyof','type K = keyof {a: 1; b: 2};\nconst k: K = "c";\nexport {};')
one('Types','indexed access type','type T = {a: number}["a"];\nconst t: T = "x";\nexport {};')
one('Types','mapped type','type M = {[K in "a" | "b"]: number};\nconst m: M = {a: 1};\nexport {};')
one('Types','conditional type','type C<T> = T extends string ? 1 : 2;\nconst c: C<string> = 2;\nexport {};')
one('Types','template literal type','type T = `a-${string}`;\nconst t: T = "b";\nexport {};')
one('Types','infer in conditional','type E<T> = T extends (infer U)[] ? U : never;\nconst e: E<number[]> = "x";\nexport {};')
one('Types','Partial / Pick utility','interface A {a: number; b: string}\nconst p: Pick<A, "a"> = {a: "x"};\nexport {};')
one('Types','Record utility','const r: Record<"a" | "b", number> = {a: 1};\nexport {};')
one('Types','namespace member','namespace N { export const a = 1 }\nN.b;\nexport {};')
# --- async / iteration
one('Async','await wrong type','async function f() { return 1 }\nasync function g() { const x: string = await f(); }\nexport {};')
one('Async','async return type','async function f(): Promise<number> { return "x"; }\nexport {};')
one('Iteration','for-of over non-iterable','for (const x of 5) {}\nexport {};')
one('Iteration','generator yield type','function* g(): Generator<number> { yield "x"; }\nexport {};')
# --- modules (cross-file)
mod('import function','export function f(x: number): string { return ""; }','import {f} from "./a";\nconst n: number = f(1);')
mod('import interface','export interface I {a: number}','import {I} from "./a";\nconst i: I = {a: "x"};')
mod('import class','export class A { x = 1 }','import {A} from "./a";\nnew A().y;')
mod('import enum','export enum E {A, B}','import {E} from "./a";\nconst e: E = "x";')
mod('import type alias','export type T = {a: number}','import type {T} from "./a";\nconst t: T = {a: "x"};')
mod('import generic function','export function id<T>(x: T): T { return x; }','import {id} from "./a";\nconst s: string = id(1);')
mod('import overloaded function','export function f(a: string): string;\nexport function f(a: number): number;\nexport function f(a: any) { return a; }','import {f} from "./a";\nf(true);')
mod('import default export','export default class A { x = 1 }','import A from "./a";\nnew A().y;')
mod('import namespace','export const a = 1;\nexport const b = "s";','import * as ns from "./a";\nconst n: string = ns.a;')
mod('re-export','export const a = 1;','export {a} from "./a";\nexport const z: string = 1;')
mod('missing export','export const a = 1;','import {nope} from "./a";')
mod('missing module','export const a = 1;','import {x} from "./missing";')
mod('const enum','export const enum E {A = 1}','import {E} from "./a";\nconst e: string = E.A;')
mod('inferred export','export const v = [1, 2, 3];','import {v} from "./a";\nconst s: string = v;')
mod('abstract class export','export abstract class A {}','import {A} from "./a";\nnew A();')
multi('Modules','circular imports',**{'a.ts':'import {b} from "./b";\nexport const a: number = 1;\nexport const c: string = b;','b.ts':'import {a} from "./a";\nexport const b: number = a;'})
# --- valid programs: tsc reports nothing; any tsox error is a false positive
V='Valid programs'
one(V,'static member read','class A { static s = 1 }\nexport const n: number = A.s;')
one(V,'static method call','class A { static make(): A { return new A(); } }\nexport const a: A = A.make();')
one(V,'class inheritance','class A { x = 1 }\nclass B extends A { y = 2 }\nexport const b: B = new B();\nexport const n: number = b.x + b.y;')
one(V,'abstract class extension','abstract class A { abstract m(): number }\nclass B extends A { m() { return 1 } }\nexport const n: number = new B().m();')
one(V,'interface implements','interface I { m(): number }\nclass C implements I { m() { return 1 } }\nexport const i: I = new C();')
one(V,'getter and setter','class A { private _v = 1; get v(): number { return this._v } set v(x: number) { this._v = x } }\nconst a = new A(); a.v = 2;\nexport const n: number = a.v;')
one(V,'enum usage','enum E { A = 1, B }\nexport const e: E = E.B;\nexport const n: number = E.A;')
one(V,'generic function and class','function id<T>(x: T): T { return x }\nclass Box<T> { constructor(public v: T) {} }\nexport const n: number = id(1) + new Box(2).v;')
one(V,'async and await','async function f() { return 1 }\nexport async function g(): Promise<number> { return (await f()) + 1 }')
one(V,'optional chaining and nullish','type O = { a?: { b?: number } }\nexport function f(o: O): number { return o.a?.b ?? 0 }')
one(V,'union and intersection','type A = { a: number } & { b: string };\nexport const x: A = { a: 1, b: "s" };\nexport const y: string | number = x.b;')
one(V,'tuple destructuring','export const t: [number, string] = [1, "a"];\nexport const [n, s] = t;\nexport const m: number = n;')
one(V,'array methods with explicit types','const a: number[] = [1, 2, 3];\nexport const b: number = a.length + a.indexOf(2);\na.push(4);')
one(V,'object with methods and this','const o = { n: 1, m() { return this.n } };\nexport const r: number = o.m();')
one(V,'namespace merge','namespace N { export const a = 1 }\nexport const n: number = N.a;')
one(V,'Partial and Readonly','interface A { a: number; b: string }\nexport const p: Partial<A> = {};\nexport const r: Readonly<A> = { a: 1, b: "s" };')
one(V,'for-of over array','const a = [1, 2];\nlet s = 0;\nfor (const x of a) { s += x }\nexport { s };')
one(V,'template literal value','const n = 1;\nexport const s: string = `n=${n}`;')
multi(V,'cross-file class with static',**{'a.ts':'export class A { static s = 1; x = 2 }','b.ts':'import {A} from "./a";\nexport const n: number = A.s + new A().x;'})
multi(V,'cross-file generic and re-export',**{'a.ts':'export function id<T>(x: T): T { return x }','b.ts':'export {id} from "./a";','c.ts':'import {id} from "./b";\nexport const n: number = id(1);'})


def codes_tsc(out):
    found = set()
    for line in out.split('\n'):
        m = re.match(r'(\S+?)\((\d+),\d+\): error (TS\d+):', line)
        if m:
            found.add((os.path.basename(m.group(1)), m.group(3)))
    return found


def codes_tsox(out):
    found, warnings = set(), 0
    for line in out.split('\n'):
        m = re.match(r'.*?([^/\s:]+\.ts):(\d+):\d+: error: (TS\d+) ', line)
        if m:
            found.add((m.group(1), m.group(3)))
        if ': warning:' in line:
            warnings += 1
    return found, warnings


def find_tsox(arg):
    for c in ([arg] if arg else []) + [os.path.join(ROOT, 'target', k, 'tsox') for k in ('release', 'debug')]:
        if c and os.path.exists(c):
            return c
    sys.exit('no tsox binary: build one (cargo build) or pass --tsox')


def find_tsc(arg):
    c = arg or os.path.join(ROOT, 'scripts', 'ts-diag-tool', 'node_modules', '.bin', 'tsc')
    if os.path.exists(c):
        return c
    found = shutil.which('tsc')
    if found:
        return found
    sys.exit('no tsc: run `npm ci` in scripts/ts-diag-tool or pass --tsc')


def run_all(tsox, tsc):
    rows = []
    with tempfile.TemporaryDirectory() as tmp:
        for i, (area, name, files) in enumerate(P):
            d = os.path.join(tmp, '%03d' % i)
            os.makedirs(d)
            with open(os.path.join(d, 'tsconfig.json'), 'w') as f:
                f.write(CONFIG)
            for fname, src in files.items():
                with open(os.path.join(d, fname), 'w') as f:
                    f.write(src + '\n')
            t = subprocess.run([tsc, '-p', os.path.join(d, 'tsconfig.json'), '--pretty', 'false'],
                               capture_output=True, text=True, timeout=120).stdout
            try:
                x = subprocess.run([tsox, '--project', os.path.join(d, 'tsconfig.json'), '--tsc-codes'],
                                   capture_output=True, text=True, timeout=60)
                xo, hang = x.stdout + x.stderr, False
            except subprocess.TimeoutExpired:
                xo, hang = '', True
            tc, (xc, warn) = codes_tsc(t), codes_tsox(xo)
            if hang:
                verdict = 'HANG'
            elif tc == xc:
                verdict = 'MATCH'
            elif not (xc - tc):
                verdict = 'GAP'
            elif not (tc - xc):
                verdict = 'FALSE-POS'
            else:
                verdict = 'DIFF'
            rows.append(dict(area=area, name=name, files=files, verdict=verdict, warnings=warn,
                             tsc=sorted({c for _, c in tc}), tsox=sorted({c for _, c in xc})))
    return rows


def markdown(rows):
    by = OrderedDict()
    for r in rows:
        by.setdefault(r['area'], []).append(r)
    total = Counter(r['verdict'] for r in rows)
    out = ['| Result | Probes |', '| --- | --- |']
    for v in ('MATCH', 'GAP', 'DIFF', 'FALSE-POS', 'HANG'):
        if total[v]:
            out.append('| %s | %d |' % (v, total[v]))
    out += ['', '| Area | Probes | Match | Gap | Diff | False positive |', '| --- | --- | --- | --- | --- | --- |']
    for area, rs in by.items():
        c = Counter(r['verdict'] for r in rs)
        out.append('| %s | %d | %d | %d | %d | %d |' % (area, len(rs), c['MATCH'], c['GAP'], c['DIFF'], c['FALSE-POS']))
    for area, rs in by.items():
        out += ['', '### ' + area, '', '| Probe | tsc | tsox | Result |', '| --- | --- | --- | --- |']
        for r in rs:
            tsc = ', '.join(r['tsc']) or 'none'
            tsox = ', '.join(r['tsox']) or 'none'
            if not r['tsox'] and r['warnings']:
                tsox += ' (warns: not checked)'
            out.append('| %s | %s | %s | %s |' % (r['name'], tsc, tsox, r['verdict']))
    return '\n'.join(out)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--tsox')
    ap.add_argument('--tsc')
    ap.add_argument('--markdown', action='store_true')
    ap.add_argument('--json')
    a = ap.parse_args()
    rows = run_all(find_tsox(a.tsox), find_tsc(a.tsc))
    if a.json:
        with open(a.json, 'w') as f:
            json.dump(rows, f, indent=1)
    if a.markdown:
        print(markdown(rows))
    else:
        print(len(rows), 'probes', dict(Counter(r['verdict'] for r in rows)))
        for r in rows:
            if r['verdict'] != 'MATCH':
                print('%-9s %-14s %-34s tsc=%s tsox=%s warnings=%d' % (r['verdict'], r['area'], r['name'], r['tsc'], r['tsox'], r['warnings']))


if __name__ == '__main__':
    main()
