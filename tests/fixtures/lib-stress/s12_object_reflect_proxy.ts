// lib.es2015.reflect / proxy; Object descriptors.
const o = { a: 1 };
const g: any = Reflect.get(o, "a");
const h: boolean = Reflect.has(o, "a");
const ks: (string | symbol)[] = Reflect.ownKeys(o);
const d: PropertyDescriptor | undefined = Object.getOwnPropertyDescriptor(o, "a");
Object.defineProperty(o, "b", { value: 2, enumerable: true });
const ds: { [k: string]: PropertyDescriptor } = Object.getOwnPropertyDescriptors(o);
const handler: ProxyHandler<{ a: number }> = {
  get(target, prop, receiver) { return Reflect.get(target, prop, receiver); },
  set(target, prop, value) { return true; },
};
const px: { a: number } = new Proxy({ a: 1 }, handler);
const rev = Proxy.revocable({ a: 1 }, {});
const same: boolean = Object.is(1, 1);
const bad1: string = Reflect.has(o, "a"); // error 2322
const bad2 = new Proxy(1, {}); // error 2345
export {};
