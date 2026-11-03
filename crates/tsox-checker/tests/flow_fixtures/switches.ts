// dispatch, fallthrough, empty case, default in the middle.
export function sw(k: number) {
  switch (k) {
    case 1:
    case 2:
      k;
      break;
    default:
      k;
    case 3:
      k;
  }
}

// the discriminant branches, so the dispatch starts in the block where its paths meet.
export function sw2(a: number | null, b: number) {
  switch (a ?? b) {
    case 1:
      a;
      break;
    default:
      b;
  }
}
