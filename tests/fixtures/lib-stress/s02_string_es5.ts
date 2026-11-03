// lib.es5 String and RegExp.
const s = "Hello, World";
const up: string = s.toUpperCase();
const parts: string[] = s.split(", ");
const ch: string = s.charAt(0);
const code: number = s.charCodeAt(0);
const rep: string = s.replace(/o/g, (m, off: number) => m + off);
const rep2: string = s.replace("l", "L");
const m: RegExpMatchArray | null = s.match(/W(or)ld/);
const first: string | undefined = m?.[0];
const idx: number = s.search(/W/);
const sub: string = s.substring(1, 3) + s.substr(1, 2) + s.slice(-2);
const trimmed: string = "  x ".trim();
const n: number = s.length;
const re = /a(b)?/g;
const r: RegExpExecArray | null = re.exec("ab");
const ok: boolean = re.test("a");
const bad: string = s.length; // error 2322
const bad2: number = s.split(","); // error 2322
export {};
