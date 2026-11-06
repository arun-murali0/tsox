type Boxed<T> = [T] extends [never] ? "empty" : "full";
const b: Boxed<never> = "full";
export {};
