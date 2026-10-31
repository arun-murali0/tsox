// Phase 10: function-expression return inference
export {};
function call<T>(fn: () => T): T { return fn(); }
const result: string = call(function () { return "value"; });
