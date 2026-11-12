// A generic alias applied to a type parameter is that parameter: Id<K> and K are the
// same type, in both directions, and through a keyof/indexed-access alias over Record.
type Id<T> = T;
function a<K extends string>(k: Id<K>): K { return k; }
function a2<K>(k: Id<K>): K { return k; }
function a3<K extends string>(k: Id<K>): string { return k; }
function a4<K extends string>(k: K): Id<K> { return k; }
declare const s: Id<string>;
const t: string = s;
type Wrap<T> = { v: T };
function w<K>(k: Wrap<K>): K { return k.v; }
function w2<K>(k: Id<Wrap<K>>): K { return k.v; }
type Keys<K extends string> = keyof Record<K, number>;
type IndexedValue<K extends string, V> = Record<K, V>[K];
function keysOf<K extends string>(key: Keys<K>): K { return key; }
function valueOf<K extends string, V>(value: IndexedValue<K, V>): V { return value; }
