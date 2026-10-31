type RecursiveBox<T> = {
  value: T;
  fixed: RecursiveBox<string>;
};

function readFixed(box: RecursiveBox<number>): string {
  return box.fixed.value;
}

function rejectWrongFixedValue(box: RecursiveBox<number>): number {
  return box.fixed.value;
}
