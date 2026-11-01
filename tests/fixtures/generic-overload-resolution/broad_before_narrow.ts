declare function select(value: string | number): "broad";
declare function select(value: string): "narrow";

// TypeScript preserves declaration order when both overloads are applicable.
const result: "broad" = select("text");
