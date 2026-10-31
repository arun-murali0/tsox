// Expected error: required id property missing
export {};
function needsId<T extends { id: number }>(value: T): T { return value; }
needsId({ name: "missing" });
