export {};

interface Animal { x: number }
interface Giraffe extends Animal { neck: number }
interface Elephant extends Animal { trunk: number }
declare const animal: Animal;
declare const giraffe: Giraffe;
declare const elephant: Elephant;
declare function f<T extends U, U extends Animal>(a: T, b: T, bound: U): T;
const ok: Giraffe = f(giraffe, giraffe, animal);
