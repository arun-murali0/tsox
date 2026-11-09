export const p = new Promise<number>((resolve) => {
  resolve("x"); // error TS2345
});
export const s = new Promise<number>((res) => { res(); }); // error TS2794
export const ok = new Promise<void>((res) => res());
let a: number, b: string;
[a, b] = ["x", 1]; // error TS2322
export { a, b };
