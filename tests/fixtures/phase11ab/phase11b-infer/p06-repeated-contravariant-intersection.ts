type ArgumentOfBoth<T> = T extends { left: (value: infer U) => void; right: (value: infer U) => void } ? U : never;
type Result = ArgumentOfBoth<{ left: (value: { left: string }) => void; right: (value: { right: number }) => void }>;
const good: Result = { left: 'x', right: 1 };
// @ts-expect-error contra candidates combine as an intersection
const bad: Result = { left: 'x' };
