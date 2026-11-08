export function loop(): void {
  while (Math.random() > 0.5) {
    const x = 1;
    if (x == 1) { break; }
    if (x == 2) { continue; }
  }
}
