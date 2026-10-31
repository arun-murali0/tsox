type RecursiveBox<T> = {
  value: T;
  fixed: RecursiveBox<string>;
};

declare const box: RecursiveBox<number>;
const wrong: number = box.fixed.value;
