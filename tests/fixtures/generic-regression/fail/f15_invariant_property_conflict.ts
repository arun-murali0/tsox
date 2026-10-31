// Expected error: incompatible mutable property types
export {};
function keep<T>(left: { value: T }, right: { value: T }): T { return left.value; }
keep({ value: 1 }, { value: "x" });
