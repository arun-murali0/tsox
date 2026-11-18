// Problem: tsox only checks. Producing JavaScript needs the types stripped and the result
// printed with a source map, and none of that should wait for, or depend on, the checker: tsc
// itself emits a file whose types have errors unless `noEmitOnError` says otherwise.
// Picked: a separate crate that parses with Oxc, runs Oxc's transformer to strip types, and
// prints with Oxc's code generator. It reads the semantic model only for the two things that
// need type knowledge (import elision and `const enum` inlining, in `model_pass`); with no
// model it is a syntactic strip.
// Cost: whether to keep Oxc's transformer or write an own pass is still open (proposal, open
// decision 5); this crate keeps that choice behind `emit_js` so callers do not change.

use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};
use thiserror::Error;
use tsox_model::SemanticModel;

#[derive(Clone, Debug, Default)]
pub struct EmitOptions {
    /// Produce a source map next to the code.
    pub source_map: bool,
    /// The module the file is, which names its enum members in the model. Without it the model's
    /// constant values cannot be looked up and `const enum` references are left alone.
    pub module: Option<tsox_base::keys::ModuleKey>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EmitOutput {
    pub code: String,
    /// The source map as JSON, when asked for.
    pub map: Option<String>,
}

#[derive(Debug, Error)]
pub enum EmitError {
    #[error("the file could not be parsed: {0}")]
    Parse(String),
    #[error("the transform failed: {0}")]
    Transform(String),
}

/// Strips the types from one TypeScript file and prints JavaScript. `model` is what the checker
/// knows about the file; `None` means a purely syntactic strip.
pub fn emit_js(
    source: &str,
    file_name: &str,
    options: &EmitOptions,
    model: Option<&SemanticModel>,
) -> Result<EmitOutput, EmitError> {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(file_name)
        .unwrap_or_else(|_| SourceType::default().with_typescript(true));
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.diagnostics.has_errors() {
        return Err(EmitError::Parse(
            parsed
                .diagnostics
                .errors()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    let mut program = parsed.program;
    if let Some(model) = model {
        model_pass::apply(&allocator, &mut program, model, options.module);
    }
    let scoping = SemanticBuilder::new()
        .with_enum_eval(true)
        .build(&program)
        .semantic
        .into_scoping();
    let path = std::path::Path::new(file_name);
    let transformed = Transformer::new(&allocator, path, &TransformOptions::default())
        .build_with_scoping(scoping, &mut program);
    if transformed.diagnostics.has_errors() {
        return Err(EmitError::Transform(
            transformed
                .diagnostics
                .errors()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    let printed = Codegen::new()
        .with_options(CodegenOptions {
            source_map_path: options.source_map.then(|| path.to_path_buf()),
            ..CodegenOptions::default()
        })
        .build(&program);
    Ok(EmitOutput {
        code: printed.code,
        map: printed.map.map(|map| map.to_json_string()),
    })
}

mod model_pass;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_are_stripped_and_values_stay() {
        let out = emit_js(
            "interface P { x: number }\nconst n: number = 1;\nexport function f(p: P): number { return p.x + n; }\n",
            "a.ts",
            &EmitOptions::default(),
            None,
        )
        .unwrap();
        assert!(!out.code.contains("interface"), "{}", out.code);
        assert!(!out.code.contains(": number"), "{}", out.code);
        assert!(out.code.contains("function f(p)"), "{}", out.code);
        assert!(out.map.is_none());
    }

    #[test]
    fn a_source_map_is_produced_when_asked_for() {
        let out = emit_js(
            "export const a: string = 'x';\n",
            "b.ts",
            &EmitOptions {
                source_map: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert!(out.map.is_some_and(|map| map.contains("mappings")));
    }

    fn module() -> tsox_base::keys::ModuleKey {
        tsox_base::keys::ModuleKey::builtin()
    }

    fn with_enum(values: &[(&str, &str, tsox_model::ConstValue)]) -> SemanticModel {
        let mut model = SemanticModel::default();
        for (e, m, v) in values {
            model
                .const_values
                .insert(tsox_model::enum_member_key(module(), e, m), v.clone());
        }
        model
    }

    #[test]
    fn a_const_enum_reference_is_replaced_by_its_value_and_the_declaration_goes() {
        use tsox_model::ConstValue;
        let model = with_enum(&[
            ("E", "A", ConstValue::number(0.0)),
            ("E", "B", ConstValue::number(-2.0)),
            ("E", "S", ConstValue::String("x".into())),
        ]);
        let out = emit_js(
            "const enum E { A, B = -2, S = 'x' }\nexport const v = [E.A, E.B, E['S']];\n",
            "c.ts",
            &EmitOptions {
                module: Some(module()),
                ..Default::default()
            },
            Some(&model),
        )
        .unwrap();
        assert!(!out.code.contains("E."), "{}", out.code);
        assert!(!out.code.contains("var E"), "{}", out.code);
        assert!(
            out.code.contains("-2") && out.code.contains("\"x\""),
            "{}",
            out.code
        );
    }

    #[test]
    fn an_exported_const_enum_keeps_its_declaration() {
        use tsox_model::ConstValue;
        let model = with_enum(&[("E", "A", ConstValue::number(0.0))]);
        let out = emit_js(
            "export const enum E { A }\nexport const v = E.A;\n",
            "d.ts.ts",
            &EmitOptions {
                module: Some(module()),
                ..Default::default()
            },
            Some(&model),
        )
        .unwrap();
        assert!(out.code.contains("E"), "{}", out.code);
        assert!(out.code.contains("v = 0"), "{}", out.code);
    }

    #[test]
    fn an_import_the_model_calls_type_only_is_dropped_and_a_bare_import_stays() {
        let mut model = SemanticModel::default();
        model.module_info.imports = vec![
            ("./types".into(), tsox_model::ImportUse::TypeOnly),
            ("./effect".into(), tsox_model::ImportUse::Value),
        ];
        let out = emit_js(
            "import { T } from './types';\nimport './effect';\nexport const n = 1;\n",
            "i.ts",
            &EmitOptions::default(),
            Some(&model),
        )
        .unwrap();
        assert!(!out.code.contains("./types"), "{}", out.code);
        assert!(out.code.contains("./effect"), "{}", out.code);
    }

    #[test]
    fn without_a_model_a_const_enum_is_left_to_the_transformer() {
        let out = emit_js(
            "const enum E { A }\nexport const v = E.A;\n",
            "n.ts",
            &EmitOptions::default(),
            None,
        )
        .unwrap();
        assert!(out.code.contains("E"), "{}", out.code);
    }
}
