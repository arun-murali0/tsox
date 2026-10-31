// `bigint` is not something this checker's type resolver understands yet, so
// this bound cannot be resolved and T is left unconstrained.
function first<T extends bigint>(item: T): T {
  return item;
}
