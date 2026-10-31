type ElementOf<T> = T extends (infer U)[] ? U : never;
type Leaf = ElementOf<'leaf'[]>;
const good: 'leaf' = null as unknown as Leaf;
