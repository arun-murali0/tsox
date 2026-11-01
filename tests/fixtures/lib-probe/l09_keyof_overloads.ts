// DOM addEventListener: keyof-keyed overload plus a string fallback.
interface EvMap { click: { x: number }; key: { code: string }; }
interface Target {
    on<K extends keyof EvMap>(type: K, cb: (ev: EvMap[K]) => void): void;
    on(type: string, cb: (ev: any) => void): void;
}
declare const t: Target;
t.on("click", e => { const n: number = e.x; });
t.on("click", e => { const s: string = e.x; }); // error 2322
export {};
