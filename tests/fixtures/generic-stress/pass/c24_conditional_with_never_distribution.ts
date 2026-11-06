type NoNever<T> = T extends never ? 1 : 2;
type Z = NoNever<never>; const z: Z = null as never;
type Boxed<T> = [T] extends [never] ? "empty" : "full";
const b: Boxed<never> = "empty";
export {};
