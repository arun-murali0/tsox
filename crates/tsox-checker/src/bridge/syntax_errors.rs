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
    if message.starts_with("Expected `") && tsc_says_something_else(source, message, span) {
        return None;
    }
    Some(SyntaxDiagnostic {
        span,
        message: translated,
    })
}

// Where oxc's "Expected X but found Y" is not tsc's TS1005. Each case was compared with tsc: an
// optional (`?`) or definite (`!`) object member is TS1162 or TS1255, an enum member followed by
// something other than `,`, `=` or `}` is TS1357, an empty or broken heritage clause is
// TS1097 or TS1009, a function with no name is TS1003, and a comma in a computed name is TS1171.
fn tsc_says_something_else(source: &str, message: &str, span: Span) -> bool {
    let at = span.start as usize;
    if message.ends_with("but found `?`") || message.ends_with("but found `!`") {
        return true;
    }
    if message.starts_with("Expected `,`")
        && (message.ends_with("but found `:`") || message.ends_with("but found `;`"))
    {
        return inside_enum(source, at);
    }
    if message.starts_with("Expected `{`") {
        // A class header with a broken heritage clause: `class C extends {`, `class C extends A, {`.
        if let Some(header) = source.get(..at).and_then(|before| {
            let class = before.rfind("class")?;
            Some(&before[class..])
        }) {
            let first_brace = header.find('{').unwrap_or(header.len());
            let before_brace = header[..first_brace].trim_end();
            let broken_end = first_brace == header.len()
                || ["extends", "implements", ","]
                    .iter()
                    .any(|end| before_brace.ends_with(end));
            return broken_end && (header.contains("extends") || header.contains("implements"));
        }
    }
    if message.starts_with("Expected `(`") {
        return source
            .get(..at)
            .is_some_and(|before| before.trim_end().ends_with("function"));
    }
    message.starts_with("Expected `]`") && message.ends_with("but found `,`")
}

/// Whether the innermost `{` open before `at` is the body of an `enum`. Braces in strings and
/// comments are counted like any other, so a stray one can make this answer wrongly; the caller
/// only uses it to decline a translation.
pub(super) fn inside_enum(source: &str, at: usize) -> bool {
    let Some(before) = source.get(..at) else {
        return false;
    };
    let mut depth = 0usize;
    for (index, byte) in before.bytes().enumerate().rev() {
        match byte {
            b'}' => depth += 1,
            b'{' if depth == 0 => {
                let head = before[..index].trim_end();
                let name_start = head
                    .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
                    .map_or(0, |i| i + 1);
                let keyword = head[..name_start].trim_end();
                return !head[name_start..].is_empty()
                    && keyword.ends_with("enum")
                    && keyword[..keyword.len() - 4]
                        .chars()
                        .next_back()
                        .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '$'));
            }
            b'{' => depth -= 1,
            _ => {}
        }
    }
    false
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
        // oxc and tsc word these the same, give or take the final full stop. The parser gives up
        // on all but the last (no tree), so the diagnostic is all the file reports.
        "A rest parameter must be last in a parameter list" => same(
            Code::RestParameterNotLast,
            "A rest parameter must be last in a parameter list.",
        ),
        "An enum member cannot have a numeric name." => same(Code::EnumMemberNumericName, message),
        "Computed property names are not allowed in enums." => {
            same(Code::EnumComputedName, message)
        }
        "A rest element must be last in a destructuring pattern" => same(
            Code::RestElementNotLast,
            "A rest element must be last in a destructuring pattern.",
        ),
        "An index signature parameter cannot have a question mark." => {
            same(Code::IndexSignatureOptionalParameter, message)
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

// tsc reports its grammar errors (the checker's modifier, parameter, member and statement rules)
// only for a file whose parser reported nothing: one parse error anywhere in the file silences all
// of them. These are the tsc codes the parser itself reports.
const PARSE_LEVEL: [u32; 8] = [1003, 1005, 1011, 1109, 1123, 1128, 1200, 1357];
// And these are the grammar errors that go quiet. Errors tsc reports from its ordinary checks
// (TS2364, TS2452, TS2462, TS2356, TS2357, TS2406, TS2487) are not silenced.
const GRAMMAR_LEVEL: [u32; 36] = [
    1014, 1015, 1019, 1020, 1021, 1028, 1029, 1030, 1031, 1034, 1038, 1042, 1070, 1071, 1089, 1098,
    1099, 1103, 1108, 1147, 1155, 1162, 1163, 1164, 1182, 1187, 1197, 1242, 1243, 1255, 1308, 1319,
    17006, 1100, 1101, 1102,
];

/// Drops the grammar errors tsc would not report: where the file has a parse error tsc reports
/// (one among `syntax`, or `unknown_parse_error` for one tsox cannot translate).
pub fn silence_grammar_errors(syntax: &mut Vec<SyntaxDiagnostic>, unknown_parse_error: bool) {
    let code = |found: &SyntaxDiagnostic| found.message.code.tsc_code();
    let parse_error = unknown_parse_error
        || syntax
            .iter()
            .any(|found| code(found).is_some_and(|c| PARSE_LEVEL.contains(&c)));
    if parse_error {
        syntax.retain(|found| !code(found).is_some_and(|c| GRAMMAR_LEVEL.contains(&c)));
    }
}

/// How many of `syntax` are errors tsc's parser reports.
pub fn parse_level_count(syntax: &[SyntaxDiagnostic]) -> usize {
    syntax
        .iter()
        .filter(|found| {
            found
                .message
                .code
                .tsc_code()
                .is_some_and(|c| PARSE_LEVEL.contains(&c))
        })
        .count()
}
