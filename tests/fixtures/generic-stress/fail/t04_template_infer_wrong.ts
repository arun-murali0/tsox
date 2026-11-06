type Parse<S> = S extends `${infer A}-${infer B}` ? [A, B] : never;
const v: Parse<"foo-bar"> = ["foo", "baz"];
export {};
