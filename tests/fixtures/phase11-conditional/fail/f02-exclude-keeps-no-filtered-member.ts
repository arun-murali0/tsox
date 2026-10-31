type ExcludeLocal<T, U> = T extends U ? never : T;
type OnlyB = ExcludeLocal<'a' | 'b', 'a'>;
const bad: OnlyB = 'a';
