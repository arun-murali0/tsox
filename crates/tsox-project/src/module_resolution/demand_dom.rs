//! Whether a project needs the DOM-family standard-library files.
//!
//! `lib.dom.d.ts` and its companions are most of the default library, and nothing in the ES
//! libraries refers to them. A project that neither uses nor declares one of their globals
//! sees the same program with or without them, so the driver can leave them out and load them
//! only when this says they are needed. The test is deliberately one-sided: anything it cannot
//! rule out counts as a need, because loading them is what every project did before.

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

use tsox_base::file_id::FileId;

use super::graph::ModuleGraph;

/// True when some file of the project, other than a bundled library file, could observe a
/// DOM-family global:
///
/// - it uses a name only those libraries declare, with no binding of its own,
/// - it names `globalThis`, whose type holds every global,
/// - it is a script that declares such a name, which would merge with the library's,
/// - it contains `declare global`, which can augment a library interface, or
/// - it cannot be parsed.
pub fn project_needs_dom_libraries(graph: &ModuleGraph) -> bool {
    // The same larger stack the checker gives the parser and the semantic builder: a chain the
    // bracket count cannot see (`else if` repeated thousands of times) still recurses in both.
    tsox_base::stack_guard::on_big_stack(|| project_needs_dom_libraries_inner(graph))
}

fn project_needs_dom_libraries_inner(graph: &ModuleGraph) -> bool {
    let library_directory = tsox_base::standard_lib::library_directory();
    (0..graph.len())
        .map(|index| FileId::new(u32::try_from(index).expect("project contains too many files")))
        .any(|id| {
            let Some(path) = graph.files().path(id) else {
                return false;
            };
            if path.starts_with(&library_directory) {
                return false;
            }
            match graph.source(id) {
                Some(source) => source_needs_dom_libraries(source, path),
                // Not kept (not UTF-8 or not scannable): no way to rule it out.
                None => true,
            }
        })
}

fn source_needs_dom_libraries(source: &str, path: &Path) -> bool {
    if source.contains("declare global") {
        return true;
    }
    // The semantic builder recurses once per level of nesting, as the parser does. A file
    // nested past the checker's limit is refused there with its own error; here it only has
    // to be kept away from a pass that would overflow the stack, and "needs DOM" is the answer
    // that changes nothing for it.
    if tsox_base::source_depth::max_bracket_depth(source)
        > tsox_base::stack_guard::MAX_BRACKET_DEPTH
    {
        return true;
    }
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.diagnostics.has_errors() {
        return true;
    }
    let is_module = parsed.module_record.has_module_syntax;
    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    let scoping = semantic.scoping();
    let is_global_name =
        |name: &str| name == "globalThis" || tsox_base::standard_lib::is_dom_only_global(name);
    if scoping
        .root_unresolved_references()
        .keys()
        .any(|name| is_global_name(name.as_str()))
    {
        return true;
    }
    // A script's top-level declarations are globals, and one named like a DOM global merges
    // with it (an `interface Event` here gains the library's members).
    if !is_module {
        let root = scoping.root_scope_id();
        return scoping
            .get_bindings(root)
            .keys()
            .any(|name| is_global_name(name.as_str()));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::source_needs_dom_libraries;
    use std::path::Path;

    fn needs(source: &str) -> bool {
        source_needs_dom_libraries(source, Path::new("a.ts"))
    }

    #[test]
    fn plain_es_code_does_not_need_dom() {
        assert!(!needs(
            "const a: number[] = [1]; const p = Promise.resolve(a.length);"
        ));
    }

    #[test]
    fn a_dom_global_use_needs_dom() {
        assert!(needs("const e = document.body;"));
        assert!(needs("let t: HTMLElement | null = null; export {};"));
        assert!(needs("setTimeout(() => {}, 1);"));
    }

    #[test]
    fn a_local_binding_with_a_dom_name_does_not_need_dom() {
        assert!(!needs(
            "export function f(name: string) { const length = name.length; return length; }"
        ));
        assert!(!needs("export interface Event { x: number }"));
    }

    #[test]
    fn a_script_declaring_a_dom_name_needs_dom() {
        assert!(needs("interface Event { extra: number }"));
        assert!(needs("var name = 1;"));
    }

    #[test]
    fn global_this_declare_global_and_syntax_errors_are_not_ruled_out() {
        assert!(needs("export const g = globalThis;"));
        assert!(needs("export {}; declare global { interface Foo {} }"));
        assert!(needs("const = ;"));
        assert!(needs(&"(".repeat(200_000)));
    }
}
