// Expected error: callback parameter is incompatible with string
export {};
function use<T>(value: T, callback: (value: T) => void): void { callback(value); }
use("x", (value: number) => {});
