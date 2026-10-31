export {};

interface Animal { x: number }
interface Giraffe extends Animal { neck: number }
interface Elephant extends Animal { trunk: number }
declare const animal: Animal;
declare const giraffe: Giraffe;
declare const elephant: Elephant;
declare function chain<A extends B, B extends C, C extends Animal>(a: A, b: B, c: C): A;
const ok: Giraffe = chain(giraffe, giraffe, animal);
