declare function project<T>(callback: (value: T) => number): "numeric";
declare function project<U>(callback: (value: U) => string): "text";

const result: "text" = project(value => "x");
