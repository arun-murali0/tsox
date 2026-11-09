const enum Enum1 { A0 = 100 }
const enum Enum1 {
  A,
  B,
  C = 10,
  D = A | B,
  W2 = Enum1.A0,
  W3 = Enum1["A0"],
  W4 = Enum1["C"],
  W5 = Enum1[`C`],
}
enum E2 { A0 = 1 }
enum E2 { B = 2, D = B | E2.A0 }
export const xs = [Enum1.A, Enum1.D, Enum1.W2, Enum1.W3, Enum1.W4, Enum1.W5, E2.A0, E2.D];
