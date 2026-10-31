// Phase 4: contextual callback typing
export {};
function mapOne<T, U>(value: T, mapper: (value: T) => U): U { return mapper(value); }
const result: number = mapOne("abc", text => text.length);
