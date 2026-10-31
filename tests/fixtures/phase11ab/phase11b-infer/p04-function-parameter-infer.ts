type ArgumentOf<T> = T extends (value: infer U) => void ? U : never;
type Result = ArgumentOf<(value: { id: number }) => void>;
const good: { id: number } = null as unknown as Result;
