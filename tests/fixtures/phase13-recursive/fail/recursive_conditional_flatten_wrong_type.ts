type Flatten<T> = T extends (infer Item)[] ? Flatten<Item> : T;
type Result = Flatten<number[][][]>;
const value: string = null as unknown as Result;
