declare function mk<T extends object = {}>(): T;
const a: {} = mk(); const b: { x: 1 } = mk<{ x: 1 }>();
export {};
