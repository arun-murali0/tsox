// lib.es2018.regexp: named groups; lib.es2015.core RegExp flags.
const re = /(?<year>\d{4})-(?<month>\d{2})/u;
const m = re.exec("2020-05");
const y: string | undefined = m?.groups?.year;
const fl: string = re.flags;
const g: boolean = re.global || re.sticky || re.unicode || re.ignoreCase;
const all = [..."a1b2".matchAll(/\d/g)].map(x => x[0]);
const rep: string = "2020-05".replace(re, "$<month>/$<year>");
const bad1: number = re.flags; // error 2322
export {};
