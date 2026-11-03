// lib.es5/es2018/es2020 Intl.
const nf = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const a: string = nf.format(1234.5);
const parts: Intl.NumberFormatPart[] = nf.formatToParts(1);
const df = new Intl.DateTimeFormat("en-US", { year: "numeric", month: "long" });
const b: string = df.format(new Date());
const ro: Intl.ResolvedDateTimeFormatOptions = df.resolvedOptions();
const col = new Intl.Collator("en");
const c: number = col.compare("a", "b");
const pr = new Intl.PluralRules("en");
const d: Intl.LDMLPluralRule = pr.select(1);
const rtf = new Intl.RelativeTimeFormat("en");
const e: string = rtf.format(-1, "day");
const bad1: number = nf.format(1); // error 2322
const bad2 = nf.format("x"); // error 2769
export {};
