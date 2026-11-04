type E = Exclude<"a" | "b" | "c", "b">; type X = Extract<string | number | (() => void), Function>;
const e: E = "a"; const e2: E = "c"; const x: X = () => {};
export {};
