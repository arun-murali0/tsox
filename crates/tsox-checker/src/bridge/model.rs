//! Facts the checker records for the semantic model (`tsox-model`) that come straight from the
//! syntax tree and Oxc's reference flags, with no type needed. Facts that need types
//! (node types, narrowed reads) are written where those types are decided, once each.
use std::collections::BTreeMap;

use oxc_ast::ast::{ImportDeclarationSpecifier, ImportOrExportKind, Program, Statement};
use oxc_semantic::Scoping;
use tsox_model::ImportUse;

/// Each module specifier the file imports, and whether the import is used as a value. This is
/// what import elision reads: an import used only as a type, or not at all, is dropped by
/// emit; one used as a value, and a bare `import "x"` (kept for its side effect), stay.
///
/// A specifier imported by several declarations is `Value` when any of them is. Written with
/// `import type`, or `import { type X }`, an import is `TypeOnly` whatever its references say.
pub(super) fn import_uses(program: &Program<'_>, scoping: &Scoping) -> Vec<(String, ImportUse)> {
    let mut uses: BTreeMap<String, ImportUse> = BTreeMap::new();
    for statement in &program.body {
        let Statement::ImportDeclaration(decl) = statement else {
            continue;
        };
        let usage = if decl.import_kind == ImportOrExportKind::Type {
            ImportUse::TypeOnly
        } else {
            match &decl.specifiers {
                None => ImportUse::Value,
                Some(specifiers) if specifiers.is_empty() => ImportUse::Value,
                Some(specifiers) => {
                    let any_value = specifiers.iter().any(|specifier| {
                        let (local, type_only) = match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(s) => {
                                (&s.local, s.import_kind == ImportOrExportKind::Type)
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(s) => {
                                (&s.local, false)
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(s) => {
                                (&s.local, false)
                            }
                        };
                        !type_only
                            && local.symbol_id.get().is_some_and(|symbol| {
                                scoping
                                    .get_resolved_references(symbol)
                                    .any(|reference| reference.is_value())
                            })
                    });
                    if any_value {
                        ImportUse::Value
                    } else {
                        ImportUse::TypeOnly
                    }
                }
            }
        };
        let entry = uses
            .entry(decl.source.value.to_string())
            .or_insert(ImportUse::TypeOnly);
        if usage == ImportUse::Value {
            *entry = ImportUse::Value;
        }
    }
    uses.into_iter().collect()
}

pub(super) const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

pub(super) fn fnv_name(hash: u64, name: &str) -> u64 {
    name.bytes().chain([0]).fold(hash, |h, byte| {
        (h ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
    })
}

/// The type of each exported declaration, named like `node_symbols` names a statement: the
/// hash of the declared name, index 0. A body holding `any` or an error type is `Unchecked`,
/// since an IR cannot rely on it without a run-time check.
pub(super) fn node_types(
    decls: &[tsox_types::portable::PortableDecl],
) -> Vec<(tsox_model::NodeKey, tsox_model::TypeFact)> {
    use tsox_base::stable_hash::Prim;
    use tsox_types::portable::PNode;
    decls
        .iter()
        .map(|decl| {
            let loose = decl
                .body
                .nodes
                .iter()
                .any(|node| matches!(node, PNode::Prim(Prim::Any) | PNode::Prim(Prim::Error)));
            (
                tsox_model::NodeKey {
                    item: fnv_name(FNV_BASIS, &decl.name),
                    index: 0,
                },
                tsox_model::TypeFact {
                    ty: decl.body.clone(),
                    trust: if loose {
                        tsox_model::Trust::Unchecked
                    } else {
                        tsox_model::Trust::Trusted
                    },
                },
            )
        })
        .collect()
}

/// What each identifier reference names, for references to a declaration the file exports.
/// A reference is named by the top-level statement it sits in (a hash of the names that
/// statement declares, never a byte offset) and its order among the references in that
/// statement. A name two exported declarations share (a type and a value, merged parts) is
/// left out, since a bare identifier does not say which one it means.
pub(super) fn node_symbols(
    semantic: &oxc_semantic::Semantic<'_>,
    program: &Program<'_>,
    decls: &[tsox_types::portable::PortableDecl],
) -> Vec<(tsox_model::NodeKey, tsox_base::keys::DeclKey)> {
    use oxc_span::GetSpan;
    let scoping = semantic.scoping();
    let root = scoping.root_scope_id();
    let mut unique: BTreeMap<&str, Option<tsox_base::keys::DeclKey>> = BTreeMap::new();
    for decl in decls {
        unique
            .entry(&decl.name)
            .and_modify(|key| *key = None)
            .or_insert(Some(decl.key));
    }
    // (statement span, hash of declared names), in source order. One pass over the root
    // symbols: each is given to the statement that contains it.
    let mut items: Vec<(oxc_span::Span, u64)> = program
        .body
        .iter()
        // FNV-1a: fixed in every run, unlike the std hasher's seeds.
        .map(|statement| (statement.span(), FNV_BASIS))
        .collect();
    let item_at = |items: &[(oxc_span::Span, u64)], span: oxc_span::Span| {
        let at = items.partition_point(|(range, _)| range.end < span.end);
        (at < items.len() && items[at].0.contains_inclusive(span)).then_some(at)
    };
    for symbol in scoping.symbol_ids() {
        if scoping.symbol_scope_id(symbol) != root {
            continue;
        }
        if let Some(at) = item_at(&items, scoping.symbol_span(symbol)) {
            items[at].1 = fnv_name(items[at].1, scoping.symbol_name(symbol));
        }
    }
    let mut found: Vec<(u64, u32, tsox_base::keys::DeclKey)> = Vec::new();
    for symbol in scoping.symbol_ids() {
        if scoping.symbol_scope_id(symbol) != root {
            continue;
        }
        let Some(Some(key)) = unique.get(scoping.symbol_name(symbol)) else {
            continue;
        };
        for reference in scoping.get_resolved_references(symbol) {
            let span = semantic.nodes().get_node(reference.node_id()).span();
            if let Some(at) = item_at(&items, span) {
                found.push((items[at].1, span.start, *key));
            }
        }
    }
    found.sort();
    let mut out = Vec::with_capacity(found.len());
    let mut previous = (u64::MAX, 0u32);
    for (item, _, key) in found {
        previous = if previous.0 == item {
            (item, previous.1 + 1)
        } else {
            (item, 0)
        };
        out.push((
            tsox_model::NodeKey {
                item,
                index: previous.1,
            },
            key,
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxc_allocator::Allocator;
    use oxc_parser::Parser;
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;

    fn uses(source: &str) -> Vec<(String, ImportUse)> {
        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, source, SourceType::ts()).parse();
        let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
        import_uses(&parsed.program, semantic.scoping())
    }

    #[test]
    fn value_type_only_and_side_effect_imports_are_told_apart() {
        let source = r#"
            import { a } from "value";
            import { T } from "types";
            import type { U } from "explicit";
            import { type V, w } from "mixed";
            import "effect";
            import { unused } from "unused";
            a(); let t: T; let u: U; let v: V;
            w;
        "#;
        let got: BTreeMap<_, _> = uses(source).into_iter().collect();
        assert_eq!(got["value"], ImportUse::Value);
        assert_eq!(got["types"], ImportUse::TypeOnly);
        assert_eq!(got["explicit"], ImportUse::TypeOnly);
        assert_eq!(got["mixed"], ImportUse::Value);
        assert_eq!(got["effect"], ImportUse::Value);
        assert_eq!(got["unused"], ImportUse::TypeOnly);
    }
}
