// lib.es5 utility types applied to real shapes.
interface P { id: number; name: string; tags?: string[] }
type A1 = Partial<P>;
type A2 = Required<P>;
type A3 = Readonly<P>;
type A4 = Pick<P, "id" | "name">;
type A5 = Omit<P, "tags">;
type A6 = Record<"x" | "y", number>;
type A7 = Exclude<"a" | "b" | "c", "b">;
type A8 = Extract<string | number | boolean, string | number>;
type A9 = NonNullable<string | null | undefined>;
type B1 = ReturnType<() => Promise<number>>;
type B2 = Parameters<(a: number, b: string) => void>;
type B3 = ConstructorParameters<typeof Date>;
type B4 = InstanceType<typeof Map<string, number>>;
type B5 = Awaited<Promise<Promise<number>>>;
type B6 = Uppercase<"ab"> | Lowercase<"CD"> | Capitalize<"ef"> | Uncapitalize<"GH">;
const p1: A1 = {};
const p2: A2 = { id: 1, name: "n", tags: [] };
const p4: A4 = { id: 1, name: "n" };
const p5: A5 = { id: 1, name: "n" };
const p6: A6 = { x: 1, y: 2 };
const p7: A7 = "a";
const p8: A8 = 1;
const p9: A9 = "s";
const q1: B1 = Promise.resolve(1);
const q2: B2 = [1, "a"];
const q3: B3 = [0];
const q5: B5 = 1;
const q6: B6 = "AB";
const e1: A2 = { id: 1, name: "n" }; // error 2741
const e2: A3 = { id: 1, name: "n" }; const e2b = (e2.id = 3); // error 2540
const e3: A7 = "b"; // error 2322
const e4: A6 = { x: 1 }; // error 2741
const e5: B1 = "s"; // error 2322
export {};
