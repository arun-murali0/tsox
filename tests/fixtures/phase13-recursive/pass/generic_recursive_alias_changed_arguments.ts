type RecursiveBox<T> = {
  value: T;
  fixed: RecursiveBox<string>;
};

declare const box: RecursiveBox<number>;
const value: number = box.value;
const fixedValue: string = box.fixed.value;
const nestedFixedValue: string = box.fixed.fixed.value;
