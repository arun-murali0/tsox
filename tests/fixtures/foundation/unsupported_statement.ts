// A `with` statement is a statement kind this checker does not handle, and it has no plan to
// be: it is not allowed in strict mode or in a module. It used to be a global augmentation,
// before that a do-while, a namespace and an ambient module, all checked now; any statement
// that still reaches push_unsupported does the job.
declare var scope: { counter: number };
with (scope) {
    counter;
}
