// Deliberate divergence from tsc, the same category as
// array_indexing_dynamic_key_includes_undefined.ts elsewhere: this checker's
// Promise<T> is an opaque, empty object (see resolve_builtin_generic's own
// doc comment) since there is no await/.then() modeling at all. tsc's real
// Promise does have .then, so tsc reports nothing here; tsox honestly
// reports the property as missing rather than silently pretending .then
// exists. Expected to show up as a FALSE POSITIVE in compare-local.sh.
function run(p: Promise<number>): void {
    p.then();
}
