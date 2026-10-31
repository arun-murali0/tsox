type User = { name: string; age: number; active: boolean };
const user: Pick<User, "name" | "active"> = { name: "Ada", active: true };
