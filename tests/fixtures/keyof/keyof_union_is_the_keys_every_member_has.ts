type A = { shared: 1; onlyA: 1 };
type B = { shared: 2; onlyB: 2 };
const ok: keyof (A | B) = 'shared';
const bad: keyof (A | B) = 'onlyA';
