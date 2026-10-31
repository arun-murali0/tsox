// Phase 2: contravariant positions
export {};
type Consumer<T> = (value: T) => void;
function consume<T>(consumer: Consumer<T>, value: T): void { consumer(value); }
consume((value: string | number) => {}, "x");
