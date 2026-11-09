export const p = new Promise<number>((resolve, reject) => {
  resolve(1);
  resolve(Promise.resolve(2));
  reject(new Error("e"));
  reject();
});
let a: number, b: string;
[a, b] = [1, "x"];
export { a, b };
