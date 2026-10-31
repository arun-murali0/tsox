function sameTime(a: unknown): boolean {
    return a instanceof Date ? a.getTime() === a.getTime() : false;
}

function isRecent(val: unknown): boolean {
    if (val instanceof Date) {
        return val.getTime() > 0;
    }
    return false;
}
