// Expected error: return type is string | number
export {};
function select(flag: boolean) { if (flag) return "yes"; return 0; }
const wrong: string = select(true);
