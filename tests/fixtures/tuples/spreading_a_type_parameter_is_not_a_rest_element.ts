type G<T extends any[], U extends any[]> = [...T, ...U];
type H = [...[any], ...[any]];
type I<T extends any[]> = [string?, ...T];
type J<T extends any[]> = [...T, ...T];
