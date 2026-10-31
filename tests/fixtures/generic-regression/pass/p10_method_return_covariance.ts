// Phase 8: method return inference
export {};
type Producer<T> = { produce(): T };
function getProduced<T>(producer: Producer<T>): T { return producer.produce(); }
const value: string = getProduced({ produce() { return "ok"; } });
