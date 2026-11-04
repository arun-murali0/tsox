type Ev = { click: { x: number }; key: { code: string } };
type Handlers = { [K in keyof Ev]: (e: Ev[K]) => void };
const h: Handlers = { click: e => e.code, key: e => e.code };
export {};
