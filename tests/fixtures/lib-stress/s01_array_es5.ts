// lib.es5 Array: callbacks, overloads, element-type flow.
const nums: number[] = [3, 1, 2];
const strs: string[] = nums.map(n => String(n));
const evens: number[] = nums.filter(n => n % 2 === 0);
const total: number = nums.reduce((a, b) => a + b, 0);
const joined: string = nums.reduce((acc, n) => acc + n, "");
const firstBig: number | undefined = nums.find(n => n > 1);
const idx: number = nums.indexOf(2);
const sliced: number[] = nums.slice(1);
const both: (number | string)[] = [...nums, ...strs];
const sorted: number[] = nums.slice().sort((a, b) => a - b);
const any1: boolean = nums.some(n => n > 2) && nums.every(n => n > 0);
nums.forEach((n, i, arr) => { const x: number = arr[i] + n; });
const cat: number[] = nums.concat([4, 5], 6);
nums.push("x"); // error 2345
const wrong: string[] = nums.map(n => n * 2); // error 2322
const noCall = nums.map(); // error 2554
export {};
