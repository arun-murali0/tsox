interface IO<T> { get(): T; set: (v: T) => void; }
declare let a: IO<"x">; declare let b: IO<string>; b = a;
export {};
