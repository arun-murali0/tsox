declare function use<T extends string>(callback: () => T): "text";
declare function use<U extends number>(callback: () => U): "numeric";

const result: "numeric" = use(() => 42);
