type IsString<T> = T extends string ? 'yes' : 'no';
const result: 'yes' | 'no' = null as unknown as IsString<any>;
