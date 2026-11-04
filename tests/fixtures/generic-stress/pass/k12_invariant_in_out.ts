interface IO<T> { get(): T; set(v: T): void; }
declare let a: IO<string>; declare let b: IO<string>; a = b;
export {};
