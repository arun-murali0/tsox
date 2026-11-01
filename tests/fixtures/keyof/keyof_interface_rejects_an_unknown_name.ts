interface User { id: number; name: string }
const ok: keyof User = 'name';
const bad: keyof User = 'age';
