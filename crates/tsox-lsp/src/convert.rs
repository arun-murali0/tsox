//! tsox diagnostics as LSP diagnostics.

use tower_lsp_server::ls_types::{Diagnostic as LspDiagnostic, DiagnosticSeverity, NumberOrString};
use tsox::{Diagnostic, Severity};

use crate::text::LineMap;

/// The `source` every diagnostic carries, so an editor can tell tsox's from another tool's.
pub const SOURCE: &str = "tsox";

/// One diagnostic of a file, read against that file's text and line map. The code is tsc's
/// (`TS2322`) where one exists and tsox's own otherwise, as `tsox --tsc-codes` prints it.
pub fn to_lsp(diagnostic: &Diagnostic, map: &LineMap, text: &str) -> LspDiagnostic {
    let code = diagnostic
        .code
        .tsc_str()
        .unwrap_or_else(|| diagnostic.code.to_string());
    LspDiagnostic {
        range: map.range(text, diagnostic.start as usize, diagnostic.end as usize),
        severity: Some(match diagnostic.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(code)),
        source: Some(SOURCE.to_owned()),
        message: diagnostic.message.clone(),
        ..Default::default()
    }
}

/// All the diagnostics of one file, in the order the checker gave them.
pub fn file_to_lsp(diagnostics: &[Diagnostic], text: &str) -> Vec<LspDiagnostic> {
    let map = LineMap::new(text);
    diagnostics
        .iter()
        .map(|diagnostic| to_lsp(diagnostic, &map, text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp_server::ls_types::Position;
    use tsox::TypeChecker;

    fn check(source: &str) -> Vec<Diagnostic> {
        TypeChecker::new()
            .check_source(source, "a.ts")
            .unwrap()
            .diagnostics
    }

    #[test]
    fn a_real_diagnostic_lands_on_its_own_text() {
        let source = "const ok = 1;\nconst n: number = 'x';\n";
        let found = check(source);
        assert!(
            !found.is_empty(),
            "the checker should reject the assignment"
        );
        let out = file_to_lsp(&found, source);
        let first = &out[0];
        assert_eq!(first.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(first.source.as_deref(), Some(SOURCE));
        assert_eq!(first.range.start.line, 1, "second line, zero-based");
        let line = source.lines().nth(1).unwrap();
        let start = first.range.start.character as usize;
        let end = first.range.end.character as usize;
        assert!(start < end && end <= line.len());
        assert!(matches!(first.code, Some(NumberOrString::String(_))));
    }

    #[test]
    fn columns_are_utf16_after_an_astral_character() {
        let source = "const s = \"\u{1F600}\"; const n: number = 'x';\n";
        let found = check(source);
        assert!(!found.is_empty());
        let out = file_to_lsp(&found, source);
        let byte = found[0].start as usize;
        let utf16 = source[..byte].encode_utf16().count() as u32;
        assert_eq!(
            out[0].range.start,
            Position {
                line: 0,
                character: utf16
            }
        );
        assert_ne!(utf16 as usize, byte, "the emoji makes the two differ");
    }

    #[test]
    fn warnings_keep_their_severity() {
        let map = LineMap::new("x");
        let mut d = check("const n: number = 'x';").remove(0);
        d.severity = Severity::Warning;
        assert_eq!(
            to_lsp(&d, &map, "x").severity,
            Some(DiagnosticSeverity::WARNING)
        );
    }
}
