import type { HasId } from "./lib_conditional";
export const a: HasId<{ id: "abc" }> = "abc";
export const b: HasId<{ id: string }> = "any string";
