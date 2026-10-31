type IsString<T> = T extends string ? true : false;
const yes: true = null as unknown as IsString<'hello'>;
const no: false = null as unknown as IsString<42>;
