// One bad lib usage must not hide the correct errors around it.
const m = new Map<string, number>();
m.set(1, 2); // error 2345
const ok1: number | undefined = m.get("a");
const p: Promise<string> = Promise.resolve(1); // error 2322
const ok2: Promise<number> = Promise.resolve(1);
const a = [1, 2].map(x => x.nope); // error 2339
const ok3: number[] = [1, 2].map(x => x + 1);
const s = "abc".nope(); // error 2339
const ok4: string = "abc".toUpperCase();
const d = new Date().nope; // error 2339
const ok5: number = new Date().getTime();
export {};
