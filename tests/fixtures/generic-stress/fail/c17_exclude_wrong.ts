type E = Exclude<"a" | "b" | "c", "b">;
const e: E = "b";
export {};
