function first(x: string | number[]): string | number {
    if (x instanceof Array) {
        return x[0] ?? 0;
    }
    return x;
}
