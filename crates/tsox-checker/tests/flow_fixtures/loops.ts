// while with break, for, do-while: loop heads, back edges, the exit condition.
export function loops(n: number, s: string | undefined) {
  while (n > 0) {
    n--;
    if (n == 3) break;
    s;
  }
  for (let i = 0; i < n; i++) {
    i;
  }
  do {
    n;
  } while (s);
  return n;
}
