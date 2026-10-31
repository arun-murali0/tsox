// Return union from multiple branches
export {};
function select(flag: boolean) { if (flag) return "yes"; return 0; }
const result: string | number = select(true);
