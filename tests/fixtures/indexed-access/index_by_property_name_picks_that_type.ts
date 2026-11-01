type Point = { x: number; y: string };
const ok: Point['x'] = 1;
const bad: Point['x'] = 'a';
