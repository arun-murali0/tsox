// Callback returning a value can be used where void is expected
export {};
type Handler = (value: string) => void;
const handler: Handler = value => value.length;
