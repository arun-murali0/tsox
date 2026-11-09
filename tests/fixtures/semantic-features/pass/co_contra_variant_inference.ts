declare function foo<T>(x: { kind: T }, f: (arg: { kind: T }) => void): void;

declare const a: { kind: "a" };
declare const fab: (arg: { kind: "a" | "b" }) => void;
declare const fa: (arg: { kind: "a" }) => void;

foo(a, fab);
foo(a, fa);

declare function bar<T>(x: T[], f: (arg: T) => void): void;
declare const nums: number[];
bar(nums, (n) => n.toFixed());
