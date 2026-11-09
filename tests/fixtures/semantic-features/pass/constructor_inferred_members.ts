export class A {
  x;
  y;
  accessor z;
  constructor(seed: number) {
    this.x = [seed];
    this["y"] = `${seed}`;
    this.z = seed;
  }
}
