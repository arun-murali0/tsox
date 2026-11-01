// ES5 `declare var JSON: JSON`: one name is both a type and a value.
interface Jay { parse(text: string): any; stringify(v: any): string; }
declare var Jay: Jay;
const text: string = Jay.stringify({});
const bad: number = Jay.stringify({}); // error 2322
export {};
