// Expected error: Box<number> cannot satisfy Box<string>
export {};
type Box<T> = { value: T };
function unwrap<T>(box: Box<T>): T { return box.value; }
const wrong: string = unwrap({ value: 42 });
