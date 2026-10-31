// Expected error: getAge must return number
export {};
type Getters<T> = { [K in keyof T as K extends string ? `get${Capitalize<K>}` : never]: () => T[K] };
type G = Getters<{ name: string; age: number }>;
const getters: G = { getName: () => "n", getAge: () => "old" };
