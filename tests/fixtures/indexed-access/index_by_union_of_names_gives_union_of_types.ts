type Point = { x: number; y: string };
const one: Point['x' | 'y'] = 1;
const two: Point['x' | 'y'] = 'a';
const bad: Point['x' | 'y'] = true;
