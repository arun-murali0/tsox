import type { HasId } from "./lib_conditional";
export const a: HasId<{ id: "abc" }> = "xyz";
export const b: HasId<{ id: string }> = 1;
