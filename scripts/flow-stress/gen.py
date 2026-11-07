#!/usr/bin/env python3
"""Seeded generator of small, valid TypeScript files full of control flow, for comparing
tsox with tsc (`run.sh`). Every statement sits on its own line, so the line-by-line compare
points at the construct that differs.

  gen.py OUTDIR [--seed N] [--files N] [--mode reach|assign|return|narrow]

The files are valid on purpose (no `break` outside a loop, no impossible comparisons), so any
diagnostic that differs is about flow, not about a mistake in the generator.
"""
import argparse, os, random

COND = ["true", "false", "c", "(true)", "!true", "!false", "1", "0", "true && c", "c || true",
        "c && false", "null", "!c", "c && d", "c || d",
        "!(true && c)", "!(c || true)", "!(!c)", "((true))", "(true || c) && d", "c ? true : false",
        "!!true", "nb ?? true", "nb ?? false", "(nb ?? c) && false", "!(false || false)",
        "true && true", "false || false", "(c && false) || d", "c && (d || true)", "!(c && false)",
        "true as boolean", "void 0", "undefined"]

class G:
    def __init__(self, rnd, mode):
        self.r = rnd
        self.mode = mode
        self.n = 0
        self.label_n = 0
        self.vars = []  # (name, type) of the assignment-tracked variables in scope
        self.ret_values = []  # expressions the current function may return (return mode)
        # `return` is only valid inside a function; the top level and a static block are not.
        self.in_func = False
        # One discriminant per switch, declared at the top of the file. Two switches on the same
        # variable would let an earlier one narrow it to a literal that a later one exhausts.
        self.switch_vars = []

    def mark(self):
        self.n += 1
        return f"m({self.n});"

    def cond(self):
        if self.mode == "assign" and self.vars and self.r.random() < 0.4:
            v, t = self.r.choice(self.vars)
            kind = "string" if t == "string" else "number"
            return self.r.choice([
                v, f"!{v}", f"{v} !== undefined", f"{v} === undefined", f"{v} != null",
                f"{v} == null", f'typeof {v} === "{kind}"', f'typeof {v} !== "undefined"',
                f'typeof {v} === "undefined"',
            ])
        return self.r.choice(COND)

    VALUE = {"number": "1", "string": '"s"', "number | undefined": "1"}

    def read(self, v, t):
        return {"number": f"m({v});", "string": f"ms({v});",
                "number | undefined": f"mz({v});"}[t]

    # Statements that assign, read and re-declare the tracked variables.
    def assign_stmt(self, depth, loop, switch, labels):
        r = self.r
        v, t = r.choice(self.vars)
        val = self.VALUE[t]
        k = r.choice(["assign", "assign", "read", "read", "read", "inc", "condassign",
                      "ternassign", "closure", "fndecl", "iife", "destructure", "local"])
        if k == "assign":
            return [f"{v} = {val};"]
        if k == "read":
            return [self.read(v, t)]
        if k == "inc":
            return [r.choice([f"{v}++;", f"{v} += 1;"])] if t == "number" else [f"{v} = {val};"]
        if k == "condassign":
            return [f"{r.choice(['c && ', 'c || ', 'd && '])}({v} = {val});"]
        if k == "ternassign":
            return [f"c ? ({v} = {val}) : ({v} = {val});"]
        if k == "closure":
            return [f"const g{self.fresh()} = () => {{ {self.read(v, t)} }};"]
        if k == "fndecl":
            return [f"function g{self.fresh()}() {{ {self.read(v, t)} }}"]
        if k == "iife":
            return [f"(() => {{ {v} = {val}; }})();"]
        if k == "destructure":
            if t == "number":
                return [r.choice([f"[{v}] = [1];", f"({{ a: {v} }} = {{ a: 1 }});"])]
            return [f"{v} = {val};"]
        # A variable of its own inside a block: declared again on every pass of a loop.
        name = f"t{self.fresh()}"
        self.vars.append((name, "number"))
        inner = self.block(depth + 1, loop, switch, labels, count=r.randint(1, 4))
        self.vars.pop()
        return ["{", f"  let {name}: number;"] + self.ind(inner) + ["}"]

    def block(self, depth, loop, switch, labels, count=None):
        out = []
        for _ in range(count if count is not None else self.r.randint(1, 4)):
            out += self.stmt(depth, loop, switch, labels)
        return out

    def ind(self, lines):
        return ["  " + l for l in lines]

    def stmt(self, depth, loop, switch, labels):
        if self.mode == "assign" and self.vars and self.r.random() < 0.5:
            return self.assign_stmt(depth, loop, switch, labels)
        if self.mode == "return" and self.in_func and self.r.random() < 0.2:
            # A function returns the values its declared type allows; one that returns nothing
            # has a bare `return;` here and there.
            if self.ret_values and self.r.random() < 0.9:
                return [f"return {self.r.choice(self.ret_values)};"]
            return ["return;"]
        r = self.r
        simple = ["mark", "mark", "mark", "throw", "decl", "empty", "funcdecl", "exprcond",
                  "ternary", "vardecl", "classdecl", "typedecl", "enumdecl", "debugger"]
        if self.in_func:
            simple += ["return", "return"]
        if loop or switch:
            simple += ["break", "break"]
        if loop:
            simple += ["continue"]
        if labels:
            simple += ["breaklabel"]
            if any(is_loop for _, is_loop in labels):
                simple += ["continuelabel"]
        compound = ["if", "ifelse", "while", "dowhile", "forever", "forcond", "forof", "forin",
                    "switch", "try", "labeled", "block", "arrow", "iife", "iife", "static",
                    "labelchain"]
        pool = simple if depth >= 3 else simple + compound * 2
        k = r.choice(pool)
        m = self.mark
        if k == "mark": return [m()]
        if k == "return": return ["return;"]
        if k == "throw": return ["throw new Error();"]
        if k == "break": return ["break;"]
        if k == "continue": return ["continue;"]
        if k == "breaklabel": return [f"break {r.choice(labels)[0]};"]
        if k == "decl": return [f"const v{self.fresh()} = 1;"]
        if k == "empty": return [";"]
        if k == "debugger": return ["debugger;"]
        if k == "continuelabel":
            return [f"continue {r.choice([n for n, is_loop in labels if is_loop])};"]
        if k == "funcdecl": return [f"function h{self.fresh()}() {{ {m()} }}"]
        if k == "exprcond":
            cond = self.cond()
            # `??` may not be mixed with `&&` or `||` without parentheses.
            if "??" in cond:
                cond = f"({cond})"
            return [f"{cond} && {m()[:-1]};"]
        if k == "ternary": return [f"{self.cond()} ? {m()[:-1]} : {m()[:-1]};"]
        if k == "vardecl": return [f"var w{self.fresh()};"]
        if k == "classdecl": return [f"class K{self.fresh()} {{ x = 1; }}"]
        if k == "typedecl": return [f"type T{self.fresh()} = number;"]
        if k == "enumdecl": return [f"{r.choice(['enum','const enum'])} E{self.fresh()} {{ A }}"]
        d = depth + 1
        if k == "if":
            return [f"if ({self.cond()}) {{"] + self.ind(self.block(d, loop, switch, labels)) + ["}"]
        if k == "ifelse":
            return ([f"if ({self.cond()}) {{"] + self.ind(self.block(d, loop, switch, labels)) +
                    ["} else {"] + self.ind(self.block(d, loop, switch, labels)) + ["}"])
        if k == "while":
            return [f"while ({self.cond()}) {{"] + self.ind(self.block(d, True, switch, labels)) + ["}"]
        if k == "dowhile":
            return ["do {"] + self.ind(self.block(d, True, switch, labels)) + [f"}} while ({self.cond()});"]
        if k == "forever":
            return ["for (;;) {"] + self.ind(self.block(d, True, switch, labels)) + ["}"]
        if k == "forcond":
            return [f"for (let i = 0; {self.cond()}; i++) {{"] + self.ind(self.block(d, True, switch, labels)) + ["}"]
        if k == "forof":
            return ["for (const x of arr) {"] + self.ind(self.block(d, True, switch, labels)) + ["}"]
        if k == "forin":
            return ["for (const k in obj) {"] + self.ind(self.block(d, True, switch, labels)) + ["}"]
        if k == "switch":
            # One discriminant per nesting depth: a switch inside a case of a switch on the same
            # variable would see it narrowed to a literal and could exhaust it, which tsc then
            # reports as dead code after the switch. That is a type question, not a structural
            # one, so the structural set keeps the discriminants apart.
            var = f"sv{self.fresh()}"
            self.switch_vars.append(var)
            out = [f"switch ({var}) {{"]
            for case in range(r.randint(1, 3)):
                out.append(f"  case {case}:")
                out += self.ind(self.ind(self.block(d, loop, True, labels)))
            if r.random() < 0.6:
                out.append("  default:")
                out += self.ind(self.ind(self.block(d, loop, True, labels)))
            return out + ["}"]
        if k == "try":
            out = ["try {"] + self.ind(self.block(d, loop, switch, labels))
            has_catch = r.random() < 0.7
            has_finally = (not has_catch) or r.random() < 0.4
            if has_catch:
                out += ["} catch {"] + self.ind(self.block(d, loop, switch, labels))
            if has_finally:
                out += ["} finally {"] + self.ind(self.block(d, loop, switch, labels))
            return out + ["}"]
        if k == "labeled":
            name = f"L{self.fresh()}"
            inner_loop = r.random() < 0.5
            if inner_loop:
                body = [f"{name}: while ({self.cond()}) {{"] + self.ind(
                    self.block(d, True, switch, labels + [(name, True)])) + ["}"]
            else:
                body = [f"{name}: {{"] + self.ind(
                    self.block(d, loop, switch, labels + [(name, False)])) + ["}"]
            return body
        if k == "block":
            return ["{"] + self.ind(self.block(d, loop, switch, labels)) + ["}"]
        if k == "arrow":
            return ["(() => {"] + self.ind(self.func_body(d)) + ["})();"]
        if k == "iife":
            open_, close = r.choice([
                ("(() => {", "})();"),
                ("(function () {", "})();"),
                ("(async () => {", "})();"),
                ("(function* () {", "})();"),
                ("(async function () {", "})();"),
                ("(() => {", "}).call(undefined);"),
                ("void (() => {", "})();"),
                ("!function () {", "}();"),
            ])
            return [open_] + self.ind(self.func_body(d)) + [close]
        if k == "static":
            name = f"S{self.fresh()}"
            saved = self.in_func
            self.in_func = False
            body = self.block(d, False, False, [])
            self.in_func = saved
            return [f"class {name} {{", "  static {"] + self.ind(self.ind(body)) + ["  }", "}"]
        if k == "namespace":
            name = f"N{self.fresh()}"
            saved = self.in_func
            self.in_func = False
            body = self.block(d, False, False, [])
            self.in_func = saved
            return [f"namespace {name} {{", "  export const x = 1;"] + self.ind(body) + ["}"]
        if k == "labelchain":
            a, b = f"A{self.fresh()}", f"B{self.fresh()}"
            return ([f"{a}: {b}: while ({self.cond()}) {{"] +
                    self.ind(self.block(d, True, switch, labels + [(a, True), (b, True)])) + ["}"])
        raise AssertionError(k)

    def func_body(self, d):
        saved = self.in_func
        self.in_func = True
        body = self.block(d, False, False, [])
        self.in_func = saved
        return body

    def fresh(self):
        self.label_n += 1
        return self.label_n

PRELUDE = """declare function m(n: number): void;
declare function ms(s: string): void;
declare function mz(n: number | undefined): void;
declare const c: boolean;
declare const d: boolean;
declare const nb: boolean | null;
declare const arr: number[];
declare const obj: { a: number };
"""

def make_assign_file(rnd, g, idx):
    out = []
    for f in range(rnd.randint(3, 6)):
        keyword = rnd.choice(["let", "let", "var"])
        # A `var` may be declared at the end: a read above it is still a read before assigning.
        late = keyword == "var" and rnd.random() < 0.3
        decls = [f"{keyword} x: number;", f"{keyword} y: string;",
                 f"{keyword} z: number | undefined;", f"{keyword} k!: number;"]
        g.vars = [("x", "number"), ("y", "string"), ("z", "number | undefined"), ("k", "number")]
        g.in_func = True
        body = g.block(0, False, False, [], count=rnd.randint(3, 8))
        g.in_func = False
        out.append(f"function f{idx}_{f}() {{")
        out += g.ind([] if late else decls) + g.ind(body) + g.ind(decls if late else [])
        out.append("}")
    g.vars = []
    out.append("export {};")
    declares = [f"declare const {v}: number;" for v in g.switch_vars]
    return "\n".join([PRELUDE.rstrip()] + declares + out) + "\n"

# (declared return type or None, values the body may return)
RETURN_PROFILES = [
    ("number", ["1"]), ("string", ['"s"']), ("number | undefined", ["1", "undefined"]),
    ("void", []), ("any", ["1"]), ("never", []), ("boolean", ["true"]), ("unknown", ["1"]),
    ("number | void", ["1"]), ("undefined", ["undefined"]),
    (None, ["1"]), (None, ['"s"']), (None, ["1", '"s"']), (None, ["undefined"]), (None, []),
    (None, ["1", "undefined"]),
]

def make_return_file(rnd, g, idx):
    out = []
    for f in range(rnd.randint(4, 8)):
        annotation, values = rnd.choice(RETURN_PROFILES)
        kind = rnd.choice(["decl", "arrow", "expr", "method", "async_decl", "async_arrow"])
        is_async = kind.startswith("async")
        written = None
        if annotation is not None:
            written = f"Promise<{annotation}>" if is_async else annotation
        g.ret_values = values
        g.in_func = True
        body = g.block(0, False, False, [], count=rnd.randint(1, 6))
        g.in_func = False
        a = f": {written}" if written else ""
        name = f"f{idx}_{f}"
        pre = "async " if is_async else ""
        if kind in ("decl", "async_decl"):
            out += [f"{pre}function {name}(){a} {{"] + g.ind(body) + ["}"]
        elif kind in ("arrow", "async_arrow"):
            out += [f"const {name} = {pre}(){a} => {{"] + g.ind(body) + ["};"]
        elif kind == "expr":
            out += [f"const {name} = function (){a} {{"] + g.ind(body) + ["};"]
        else:
            out += [f"class C{idx}_{f} {{", f"  m(){a} {{"] + g.ind(g.ind(body)) + ["  }", "}"]
    g.ret_values = []
    out.append("export {};")
    declares = [f"declare const {v}: number;" for v in g.switch_vars]
    return "\n".join([PRELUDE.rstrip()] + declares + out) + "\n"

def make_file(rnd, mode, idx):
    g = G(rnd, mode)
    if mode == "return":
        return make_return_file(rnd, g, idx)
    if mode == "assign":
        return make_assign_file(rnd, g, idx)
    out = []
    if rnd.random() < 0.5:
        out += g.block(1, False, False, [], count=rnd.randint(1, 3))
    if rnd.random() < 0.4:
        # Namespaces are only valid at the top level of a file or of another namespace.
        name = f"N{g.fresh()}"
        out += [f"namespace {name} {{", "  export const x = 1;"] + g.ind(
            g.block(1, False, False, [], count=rnd.randint(1, 3))) + ["}"]
    for f in range(rnd.randint(2, 4)):
        out.append(f"function f{idx}_{f}() {{")
        g.in_func = True
        out += g.ind(g.block(0, False, False, [], count=rnd.randint(2, 6)))
        g.in_func = False
        out.append("}")
    out.append("export {};")
    declares = [f"declare const {v}: number;" for v in g.switch_vars]
    return "\n".join([PRELUDE.rstrip()] + declares + out) + "\n"

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--files", type=int, default=300)
    ap.add_argument("--mode", default="reach")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    for i in range(a.files):
        rnd = random.Random(a.seed * 1000003 + i)
        with open(os.path.join(a.out, f"g{a.seed}_{i:04}.ts"), "w") as fh:
            fh.write(make_file(rnd, a.mode, i))
