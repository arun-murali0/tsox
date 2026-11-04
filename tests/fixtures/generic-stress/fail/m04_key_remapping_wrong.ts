type Getters<T> = { [K in keyof T as `get${Capitalize<string & K>}`]: () => T[K] };
const g: Getters<{ name: string }> = { getName: () => 1 };
export {};
