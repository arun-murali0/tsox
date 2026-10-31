function run(fn: (() => number) | Promise<number>): number | Promise<number> {
    if (fn instanceof Promise) {
        return fn;
    }
    return fn();
}

function settle<T>(source: (() => T) | Promise<T>): T | Promise<T> {
    if (source instanceof Promise) {
        return source;
    }
    return source();
}
