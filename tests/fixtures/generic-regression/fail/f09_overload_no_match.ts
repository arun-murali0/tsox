// Expected error: no overload accepts boolean
export {};
function parse(value: string): string;
function parse(value: number): number;
function parse(value: string | number): string | number { return value; }
parse(true);
