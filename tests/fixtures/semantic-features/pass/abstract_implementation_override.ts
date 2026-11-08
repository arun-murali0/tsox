abstract class B {
  abstract prop: string;
  abstract m(): void;
}
export class C extends B {
  prop = "x";
  m() {}
}
