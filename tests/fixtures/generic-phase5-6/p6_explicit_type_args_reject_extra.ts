export {};

declare function once<T>(value: T): T;
once<string, number>("x");
