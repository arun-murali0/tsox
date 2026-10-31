export {};

declare function get<T>(o: { read(): T }): T;
const v: number = get({ read() { return 1; } });
