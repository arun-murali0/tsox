// A Promise<T> has then, catch and finally, loosely typed (their callbacks and results are
// `any`, since chaining needs generic inference), so `p.then()` is accepted as in tsc. It used
// to be reported as missing, a known false positive this fixture recorded. A member that a
// Promise does not have is still reported, once.
function run(p: Promise<number>): void {
    p.then();
    p.nope();
}
