// lib.dom: fetch, URL, storage, timers, console, structuredClone.
async function load(): Promise<void> {
  const res: Response = await fetch("/x", { method: "POST", headers: { a: "b" } });
  const j: any = await res.json();
  const t: string = await res.text();
  const ok: boolean = res.ok;
}
const u = new URL("https://a.b/c?d=1");
const q: string | null = u.searchParams.get("d");
const h: string = u.hostname;
localStorage.setItem("k", "v");
const item: string | null = localStorage.getItem("k");
const t1: number = setTimeout(() => {}, 10);
clearTimeout(t1);
const raf: number = requestAnimationFrame(t => { const n: number = t; });
console.log("x", 1);
const sc: { a: number } = structuredClone({ a: 1 });
const fd = new FormData();
fd.append("a", "b");
const ac = new AbortController();
const sig: AbortSignal = ac.signal;
const bad1: number = u.hostname; // error 2322
localStorage.setItem("k", 1); // error 2345
const bad2 = new URL(); // error 2554
export {};
