// The standard library's type-level utilities that are plain conditional aliases,
// written in TypeScript and parsed once per thread. A name the file declares itself, or
// that a loaded lib declares, is never replaced: the alias is added only when nothing
// of that name is in the namespace.
//
// Problem: `Exclude<"a" | "b", "b">`, `Awaited<T>` and the like were unknown names, so
// the annotation was left unresolved and nothing written against it was checked.
// Picked: declare them as the aliases lib.es5.d.ts writes (Awaited is the Promise-only
// form, which is what a checker without thenables needs).
// Cost: one small parse per thread, leaked for the life of the process.

use oxc_allocator::Allocator;
use oxc_ast::ast::{Statement, TSType, TSTypeParameterDeclaration};
use oxc_span::SourceType;

const SOURCE: &str = r#"
type Exclude<T, U> = T extends U ? never : T;
type Extract<T, U> = T extends U ? T : never;
type NonNullable<T> = T extends null | undefined ? never : T;
type Awaited<T> = T extends Promise<infer U> ? Awaited<U> : T;
type ReturnType<T extends (...args: any) => any> = T extends (...args: any) => infer R ? R : any;
type Parameters<T extends (...args: any) => any> = T extends (...args: infer P) => any ? P : never;
"#;

pub struct PreludeAlias {
    pub name: &'static str,
    pub body: &'static TSType<'static>,
    pub type_parameters: Option<&'static TSTypeParameterDeclaration<'static>>,
    pub span: oxc_span::Span,
}

thread_local! {
    static PRELUDE: &'static [PreludeAlias] = build();
}

fn build() -> &'static [PreludeAlias] {
    let allocator: &'static Allocator = Box::leak(Box::new(Allocator::default()));
    let parsed = oxc_parser::Parser::new(allocator, SOURCE, SourceType::ts()).parse();
    let program: &'static oxc_ast::ast::Program<'static> = allocator.alloc(parsed.program);
    let mut out = Vec::new();
    for statement in &program.body {
        if let Statement::TSTypeAliasDeclaration(alias) = statement {
            out.push(PreludeAlias {
                name: alias.id.name.as_str(),
                body: &alias.type_annotation,
                type_parameters: alias.type_parameters.as_deref(),
                span: oxc_span::Span::new(0, 0),
            });
        }
    }
    Box::leak(out.into_boxed_slice())
}

/// The prelude alias called `name`, if there is one.
pub fn alias(name: &str) -> Option<&'static PreludeAlias> {
    PRELUDE.with(|aliases| aliases.iter().find(|a| a.name == name))
}
