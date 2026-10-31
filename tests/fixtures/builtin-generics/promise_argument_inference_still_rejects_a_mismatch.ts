declare function unwrap<T>(p: Promise<T>): T;

async function load(): Promise<number> {
    return 1;
}

const s: string = unwrap(load());
