type User = { name: string; password: string };
const publicUser: Omit<User, "password"> = { name: "Ada" };
