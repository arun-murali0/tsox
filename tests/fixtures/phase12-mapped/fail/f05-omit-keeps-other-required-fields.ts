type User = { name: string; password: string };
const user: Omit<User, "password"> = {};
