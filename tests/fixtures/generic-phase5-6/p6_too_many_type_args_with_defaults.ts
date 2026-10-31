export {};

declare function withDefault<T, U = string>(value: T, other?: U): U;
withDefault<number, string, boolean>(1);
