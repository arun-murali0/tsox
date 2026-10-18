declare const pending: Promise<number>;

async function declared(): Promise<number> {
  return 1;
}
const arrowExpression = async (): Promise<number> => 1;
const arrowBlock = async (): Promise<number> => {
  return 1;
};
const functionExpression = async function (): Promise<string> {
  return "a";
};
async function generic<T>(value: T): Promise<T> {
  return value;
}
const genericArrow = async <T>(value: T): Promise<T> => value;
async function passesAPromise(): Promise<number> {
  return pending;
}
async function bareReturn(): Promise<void> {
  return;
}
async function nestedPlainFunction(): Promise<number> {
  const inner = function (): number {
    return 2;
  };
  return inner();
}
class Service {
  async load(): Promise<number> {
    return 1;
  }
  static async reset(): Promise<void> {
    return;
  }
}
