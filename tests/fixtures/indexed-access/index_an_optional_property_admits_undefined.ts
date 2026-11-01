type Opt = { a?: number };
const ok: Opt['a'] = undefined;
const also: Opt['a'] = 3;
const bad: Opt['a'] = 'no';
