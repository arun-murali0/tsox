// Shapes tsc counts as definitely assigning a property in the constructor: a
// sequence, a parenthesized or void-prefixed assignment, and a destructuring
// write through `this`. An assignment inside an `if` with no else still does
// not count.
class SequenceAssigned {
  first: number;
  second: string;
  third: boolean;
  fourth: number;
  fifth: number;

  constructor() {
    this.first = 1, this.second = "s";
    (this.third = true);
    void (this.fourth = 4);
    ({ fifth: this.fifth } = { fifth: 5 });
  }
}

class NotDefinite {
  maybe: number; // error TS2564

  constructor(flag: boolean) {
    if (flag) {
      this.maybe = 1;
    }
  }
}
