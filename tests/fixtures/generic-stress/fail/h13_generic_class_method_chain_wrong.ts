class Box<T> { constructor(public v: T) {} map<U>(f: (t: T) => U): Box<U> { return new Box(f(this.v)); } }
const b: Box<string> = new Box("abc").map(s => s.length);
export {};
