interface Cache {
    entries: Record<string, number>;
}

function run(cache: Cache): void {
    const value: string = cache.entries["anything"];
}
