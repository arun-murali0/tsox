declare function values<V>(r: Record<string, V>): V;
declare const scores: Record<string, number>;

const n: number = values(scores);
