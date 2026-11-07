type Conditional<T> = T extends number ? number : boolean;
type Box<T> = { value: T };

type Second = Conditional<string>;
const second: Second = false;
const box: Box<string> = { value: "fresh file" };
const plain: string = "active";
