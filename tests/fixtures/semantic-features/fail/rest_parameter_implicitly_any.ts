// A rest parameter with no type is TS7019 with the rest wording, in every
// parameter list a plain parameter is reported in.
function untypedRest(...args): number { // error TS7019
  return args.length;
}

const arrowRest = (...rest) => rest.length; // error TS7019
