// lib.es5 Object, JSON, Math, Number; es2017/es2019 object helpers in the default set.
const o = { a: 1, b: "x" };
const keys: string[] = Object.keys(o);
const vals: (string | number)[] = Object.values(o);
const ents: [string, string | number][] = Object.entries(o);
const frozen: Readonly<{ a: number; b: string }> = Object.freeze(o);
const copy = Object.assign({}, o, { c: true });
const c: boolean = copy.c;
const back = Object.fromEntries([["k", 1], ["j", 2]] as [string, number][]);
const n: number = back["k"];
const proto: object | null = Object.getPrototypeOf(o);
const has: boolean = Object.prototype.hasOwnProperty.call(o, "a");
const parsed: any = JSON.parse("{}");
const text: string = JSON.stringify(o, null, 2);
const mx: number = Math.max(1, 2, ...[3, 4]);
const fl: number = Math.floor(Math.random() * 10);
const fx: string = (1.234).toFixed(2);
const pi: number = parseInt("10", 10) + parseFloat("1.5") + Number("3");
const nan: boolean = Number.isNaN(1) && isNaN(1) && Number.isInteger(2);
const badKeys: number[] = Object.keys(o); // error 2322
const badMath: string = Math.max(1, 2); // error 2322
Object.freeze(o).a = 2; // error 2540
export {};
