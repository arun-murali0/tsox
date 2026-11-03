// HANG repro (lib.es2025.collection.d.ts, Set<T>): tsox does not finish. Any two of the three methods are fast.
// Expected from tsc: no errors. Run with a timeout: it does not return.
interface Other<T> { v: T; }
interface Mine<T> {
  a<U>(o: Other<U>): Mine<T | U>;
  b<U>(o: Other<U>): Mine<T & U>;
  c<U>(o: Other<U>): Mine<T | U>;
}
export {};
