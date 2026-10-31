export {};

class Box<T> { constructor(public value: T) {} }
new Box<string, number>("x");
