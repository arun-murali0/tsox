interface User { id: number; name: string }
const ok: User['name'] = 'ann';
const bad: User['id'] = 'ann';
