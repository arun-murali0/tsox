declare function run(): void;
declare let count: number;
if (run()) { count = 1; }
while (run()) { count = 2; }
const chosen = run() ? 1 : 2;
if (!run()) { count = 3; }
