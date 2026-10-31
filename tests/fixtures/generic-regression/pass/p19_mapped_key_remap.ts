// Mapped types and key remapping
export {};
type Getters<T> = { [K in keyof T as K extends string ? `get${Capitalize<K>}` : never]: () => T[K] };
type G = Getters<{ name: string; age: number }>;
const getters: G = { getName: () => "n", getAge: () => 1 };
