type Getters<T> = { [K in keyof T as `get${Capitalize<string & K>}`]: () => T[K] };
const g: Getters<{ name: string; age: number }> = { getName: () => "a", getAge: () => 1 };
export {};
