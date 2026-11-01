declare function choose<T extends { kind: "text" }>(value: { payload: T }): "text";
declare function choose<U extends { kind: "numeric" }>(value: { payload: U }): "numeric";

const result: "numeric" = choose({ payload: { kind: "numeric" } });
