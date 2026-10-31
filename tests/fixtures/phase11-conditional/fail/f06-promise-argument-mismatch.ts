type IsPromiseNumber<T> = T extends Promise<number> ? true : false;
const bad: true = null as unknown as IsPromiseNumber<Promise<string>>;
