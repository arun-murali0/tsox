type Prom<T> = { [K in keyof T]: Promise<T[K]> };
const v: Prom<[string, number]> = [Promise.resolve("a"), Promise.resolve(1)];
export {};
