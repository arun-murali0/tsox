// Problem: `bodies_phase` called seven whole-file passes by hand. Adding a pass meant editing
// that function, and nothing listed which passes exist or in what order they must run.
// Picked: one table, `PASSES`, run in table order after the statement walk. A pass is data
// (a name, a one-line doc and a function), so a contributor adds a file and one line, and a
// test can check every entry has a unique name and a doc.
// Cost: a pass that needs more than the program and its scoping still takes them through
// `CheckContext`, as before. Order is the table order and must stay: a later pass reads what an
// earlier one left in the context (the unreachable-code pass drains the flow reports).

use oxc_ast::ast::Program;
use oxc_semantic::Scoping;

use super::context::CheckContext;
use super::{
    class_implements, duplicate_members, duplicates, implicit_any, parameter_properties,
    strict_mode_names, tuple_grammar, unreachable_code, unresolved_names,
};

/// What a whole-file pass is given. Passes that need less ignore the rest.
pub(super) type PassFn =
    for<'ast, 'c> fn(&'ast Program<'ast>, &Scoping, &mut CheckContext<'ast, 'c>);

// `name` and `doc` are read by the tests and by the contributor map, not by the checker.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) struct Pass {
    pub name: &'static str,
    pub doc: &'static str,
    pub run: PassFn,
}

/// The whole-file passes, in the order they run.
pub(super) const PASSES: &[Pass] = &[
    Pass {
        name: "unresolved_names",
        doc: "Reports a type name that no declaration, import or global binds.",
        run: |program, scoping, ctx| {
            unresolved_names::report_unresolved_type_names(program, scoping, ctx);
        },
    },
    Pass {
        name: "strict_mode_names",
        doc: "Reports names that are reserved in strict mode and misuse of `eval` or `arguments`.",
        run: |program, _, ctx| strict_mode_names::report_strict_mode_names(program, ctx),
    },
    Pass {
        name: "parameter_properties",
        doc: "Reports a parameter property outside a constructor implementation.",
        run: |program, _, ctx| parameter_properties::report_parameter_properties(program, ctx),
    },
    Pass {
        name: "implicit_any",
        doc: "Reports bindings and members whose type falls back to `any` under noImplicitAny.",
        run: |program, _, ctx| implicit_any::report_implicit_any(program, ctx),
    },
    Pass {
        name: "duplicate_members",
        doc: "Reports members of one class, interface or type literal that share a name, and a repeated type parameter.",
        run: |program, _, ctx| duplicate_members::report_duplicate_members(program, ctx),
    },
    Pass {
        name: "duplicates",
        doc: "Reports duplicate declarations and conflicting merges in one scope.",
        run: |program, _, ctx| duplicates::report_duplicate_declarations(program, ctx),
    },
    Pass {
        name: "class_implements",
        doc: "Checks that a class provides what each interface in its `implements` list requires.",
        run: |program, _, ctx| class_implements::check_class_implements(program, ctx),
    },
    Pass {
        name: "tuple_grammar",
        doc: "Reports misplaced optional, rest and named elements in tuple types.",
        run: |program, _, ctx| tuple_grammar::report_tuple_grammar_errors(program, ctx),
    },
    Pass {
        name: "unreachable_code",
        doc: "Turns the flow facts into unreachable-code, fallthrough, return-path and unassigned-use reports.",
        run: |_, _, ctx| unreachable_code::check_unreachable_code(ctx),
    },
];

/// Runs every pass in table order.
pub(super) fn run_passes<'ast>(
    program: &'ast Program<'ast>,
    scoping: &Scoping,
    ctx: &mut CheckContext<'ast, '_>,
) {
    for pass in PASSES {
        (pass.run)(program, scoping, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::PASSES;

    #[test]
    fn every_pass_has_a_unique_name_and_a_doc() {
        let mut names: Vec<&str> = PASSES.iter().map(|pass| pass.name).collect();
        assert!(PASSES.iter().all(|pass| !pass.doc.trim().is_empty()));
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "pass names must be unique");
    }

    #[test]
    fn the_unreachable_code_pass_runs_last() {
        assert_eq!(
            PASSES.last().map(|pass| pass.name),
            Some("unreachable_code")
        );
    }
}
