// lib.es2015.core / es2017 / es2019 / es2020 String additions.
const s = "abc";
const a: boolean = s.startsWith("a") && s.endsWith("c") && s.includes("b");
const r: string = s.repeat(2);
const p: string = s.padStart(5, "-") + s.padEnd(5);
const t: string = " x ".trimStart() + " x ".trimEnd();
const n: string = s.normalize("NFC");
const cp: number | undefined = s.codePointAt(0);
const fcp: string = String.fromCodePoint(97, 98);
const raw: string = String.raw`a\nb`;
const mall: IterableIterator<RegExpMatchArray> = s.matchAll(/b/g);
const ea: string = s.localeCompare("b") > 0 ? "x" : "y";
const bad1: number = s.repeat(2); // error 2322
const bad2 = s.startsWith(1); // error 2345
const bad3: string = s.codePointAt(0); // error 2322
export {};
