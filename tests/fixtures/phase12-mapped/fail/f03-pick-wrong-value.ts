type User = { name: string; age: number };
const user: Pick<User, "name"> = { name: 42 };
