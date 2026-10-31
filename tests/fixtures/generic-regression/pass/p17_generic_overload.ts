// Generic overload resolution
export {};
function pick<T>(value: T): T;
function pick(value: string | number): string | number { return value; }
const text: string = pick("text");
