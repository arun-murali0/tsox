// ES5 intrinsic string types (Phase 15): the real lib's Uppercase.
const u: Uppercase<"ab"> = "AB";
const bad: Uppercase<"ab"> = "ab"; // error 2322
export {};
