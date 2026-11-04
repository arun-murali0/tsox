interface Ch { set(k: string): this; }
declare const c: Ch; const d: Ch = c.set("a").set("b");
export {};
