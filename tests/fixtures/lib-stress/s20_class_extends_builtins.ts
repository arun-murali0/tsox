// Subclassing lib classes.
class Stack<T> extends Array<T> { peek(): T | undefined { return this[this.length - 1]; } }
const s = new Stack<number>();
s.push(1);
const p: number | undefined = s.peek();
const mapped: number[] = s.map(x => x + 1);
class Counter extends Map<string, number> {
  inc(k: string): void { this.set(k, (this.get(k) ?? 0) + 1); }
}
const c = new Counter();
c.inc("a");
const cv: number | undefined = c.get("a");
class HttpError extends Error { constructor(public status: number, m: string) { super(m); } }
const he = new HttpError(404, "nf");
const st: number = he.status;
const msg: string = he.message;
class Later<T> extends Promise<T> {}
const l: Later<number> = new Later<number>(r => r(1));
const bad1: string = s.peek(); // error 2322
c.inc(1); // error 2345
const bad2 = new HttpError("x", "y"); // error 2345
export {};
