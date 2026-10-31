type User = { name: string; password: string };
const user: Omit<User, "password"> = { name: "Ada" };
const password: string = user.password;
