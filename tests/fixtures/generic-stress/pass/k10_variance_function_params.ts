type Fn<T> = (x: T) => void;
declare let a: Fn<string | number>; declare let b: Fn<string>;
b = a;
export {};
