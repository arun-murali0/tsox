const bad: Array<number> = ["a"]; // error TS2322
declare const xs: Array<string>;
xs.nope(); // error TS2339
