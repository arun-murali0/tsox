type Kind<T> = T extends string ? 'text' : 'other';
function classify<T>(value: T): Kind<T> { return null as unknown as Kind<T>; }
const result: 'text' = classify('message');
