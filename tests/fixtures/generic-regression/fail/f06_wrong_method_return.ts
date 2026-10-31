// Expected error: inferred return is string, not number
export {};
type Producer<T> = { produce(): T };
function getProduced<T>(producer: Producer<T>): T { return producer.produce(); }
const wrong: number = getProduced({ produce() { return "text"; } });
