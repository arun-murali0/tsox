type Point = { x: number; y: string };
const one: Point[keyof Point] = 1;
const two: Point[keyof Point] = 'a';
const bad: Point[keyof Point] = false;
