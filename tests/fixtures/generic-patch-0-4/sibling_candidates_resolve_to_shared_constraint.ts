export {};

interface Animal { x: number }
interface Giraffe extends Animal { neck: number }
interface Elephant extends Animal { trunk: number }
function choose<T extends Animal>(left: T, right: T): T { return left; }
declare const giraffe: Giraffe;
declare const elephant: Elephant;
const animal: Animal = choose(giraffe, elephant);
