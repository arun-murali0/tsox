function orUndefined<T extends string>(x: T): T | undefined {
    return x;
}

function orNull<T extends { id: number }>(x: T): T | null {
    return x;
}

const a: string | undefined = orUndefined("a");
const b: { id: number } | null = orNull({ id: 1 });
