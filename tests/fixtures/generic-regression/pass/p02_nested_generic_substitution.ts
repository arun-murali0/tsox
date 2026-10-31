// Phase 1: nested substitution
export {};
type Box<T> = { value: T };
function unwrap<T>(box: Box<T>): T { return box.value; }
const value: string = unwrap({ value: "hello" });
