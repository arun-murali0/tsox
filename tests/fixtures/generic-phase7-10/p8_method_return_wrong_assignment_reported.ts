export {};

declare function get<T>(o: { read(): T }): T;
const s: string = get({ read() { return 1; } });
