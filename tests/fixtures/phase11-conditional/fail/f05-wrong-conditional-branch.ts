type IsString<T> = T extends string ? true : false;
const bad: true = null as unknown as IsString<42>;
