// A rest parameter with a type annotation is not an implicit any, and neither are
// the parameters around one.
function typedRest(...args: string[]): number {
  return args.length;
}

function restAfterTyped(first: string, ...rest: number[]): number {
  return first.length + rest.length;
}
