// The parser's syntax errors, put into tsc's code and wording. oxc words each rule its own way,
// so every rule that has one tsc code and one tsc text is listed here; anything not listed is
// left unreported, since a wrong code is worse than a missing one. Rules the checker reports
// itself (the tuple shape rules, TS1039 and TS1183) are not in this table.

use oxc_span::Span;

use crate::diagnostic_codes::DiagnosticCode as Code;
use crate::diagnostic_messages::{DiagnosticMessage, messages};

pub struct SyntaxDiagnostic {
    pub span: Span,
    pub message: DiagnosticMessage,
}

/// The text between the first pair of single quotes, or backticks, of `text`.
fn quoted(text: &str) -> Option<&str> {
    let start = text.find(['\'', '`'])? + 1;
    let end = start + text[start..].find(['\'', '`'])?;
    Some(&text[start..end])
}

fn same(code: Code, message: &str) -> Option<DiagnosticMessage> {
    Some(messages::syntax(code, message.to_string()))
}

pub fn translate(source: &str, message: &str, span: Span) -> Option<SyntaxDiagnostic> {
    let translated = translate_message(message)?;
    // Two rules oxc words the same for a different tsc rule: `super<T>(...)` is TS2754, and
    // `await using` has its own codes (TS1546, TS2852).
    let after = source
        .get(span.end as usize..)
        .unwrap_or_default()
        .trim_start();
    if (message.starts_with("'super' can only") && after.starts_with('<'))
        || (message.starts_with("`await` is only") && after.starts_with("using"))
    {
        return None;
    }
    Some(SyntaxDiagnostic {
        span,
        message: translated,
    })
}

fn translate_message(message: &str) -> Option<DiagnosticMessage> {
    if let Some(rest) = message.strip_prefix("A unary expression with the ") {
        let op = quoted(rest)?;
        return same(
            Code::UnaryBeforeExponent,
            &format!(
                "An unary expression with the '{op}' operator is not allowed in the left-hand side of an exponentiation expression. Consider enclosing the expression in parentheses."
            ),
        );
    }
    if message.ends_with(" modifier must precede ")
        || (message.starts_with('\'') && message.contains("' modifier must precede '"))
    {
        return same(Code::ModifierOrder, message);
    }
    if message.starts_with('\'') && message.ends_with("' modifier already seen.") {
        return same(Code::ModifierRepeated, message);
    }
    if message.starts_with('\'')
        && message.contains("' modifier cannot be used with '")
        && message.ends_with("' modifier.")
    {
        return same(Code::ModifierConflict, message);
    }
    if message.starts_with('\'')
        && message.ends_with("' modifier cannot appear on an index signature.")
    {
        return same(Code::IndexSignatureModifier, message);
    }
    if message.starts_with("A '")
        && message.ends_with("' modifier cannot be used in an already ambient context.")
    {
        return same(Code::DeclareInAmbientContext, message);
    }
    // `Expected `)` but found `;``, and `Expected `,` or `}` but found `;``: tsc names the
    // first token it wanted.
    if let Some(rest) = message.strip_prefix("Expected `") {
        let token = &rest[..rest.find('`')?];
        // A missing `;` or `from` is where oxc and tsc most often disagree about what is
        // wrong (tsc says a keyword, an identifier or a statement was unexpected instead).
        let wanted = !token.is_empty() && !matches!(token, ";" | "from" | "in");
        if wanted && (rest.contains("` but found `") || rest.contains("` or `")) {
            return same(Code::TokenExpected, &format!("'{token}' expected."));
        }
    }
    match message {
        "Accessibility modifier already seen." => same(Code::AccessibilityRepeated, message),
        "A parameter cannot have a question mark and an initializer." => same(
            Code::OptionalParameterInitializer,
            "Parameter cannot have question mark and initializer.",
        ),
        "Missing initializer in destructuring declaration" => same(
            Code::DestructuringNeedsInitializer,
            "A destructuring declaration must have an initializer.",
        ),
        "Missing initializer in const declaration" => same(
            Code::ConstNeedsInitializer,
            "'const' declarations must be initialized.",
        ),
        "A parameter property may not be declared using a binding pattern." => {
            same(Code::ParameterPropertyPattern, message)
        }
        "Line terminator not permitted before arrow" => same(
            Code::LineBreakBeforeArrow,
            "Line terminator not permitted before arrow.",
        ),
        "Import declarations in a namespace cannot reference a module." => {
            same(Code::ImportReferencesModuleInNamespace, message)
        }
        "'super' can only be used with function calls or in property accesses" => same(
            Code::SuperNeedsAccess,
            "'super' must be followed by an argument list or member access.",
        ),
        "A 'yield' expression is only allowed in a generator body." => {
            same(Code::YieldOutsideGenerator, message)
        }
        "A 'return' statement can only be used within a function body." => {
            same(Code::ReturnOutsideFunction, message)
        }
        "Type parameter list cannot be empty." => same(Code::EmptyTypeParameterList, message),
        "Type argument list cannot be empty." => same(Code::EmptyTypeArgumentList, message),
        "A default export can only be used in an ECMAScript-style module." => {
            same(Code::DefaultExportNeedsModule, message)
        }
        "`await` is only allowed within async functions and at the top levels of modules" => same(
            Code::AwaitOutsideAsync,
            "'await' expressions are only allowed within async functions and at the top levels of modules.",
        ),
        "`for await` loops are only allowed within async functions and at the top levels of modules" => {
            same(
                Code::ForAwaitOutsideAsync,
                "'for await' loops are only allowed within async functions and at the top levels of modules.",
            )
        }
        _ => None,
    }
}
