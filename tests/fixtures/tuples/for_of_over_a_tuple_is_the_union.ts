declare const t: [number, string];
for (const v of t) {
    const ok: number | string = v;
    const bad: number = v;
}
