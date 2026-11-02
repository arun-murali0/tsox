use crate::diagnostic_codes::DiagnosticCode;
use crate::diagnostics::{Diagnostic, Severity};
use crate::line_index::LineIndex;

/// A line/column position. `Diagnostic` itself stores raw byte offsets (see
/// diagnostics.rs for why); this is the presentation-layer conversion,
/// built lazily once a LineIndex is available.
pub struct Position {
    pub line: u32,
    pub column: u32,
}

pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// A diagnostic converted to line/column, structured for display or for a
/// future LSP-shaped consumer: code, severity, range, and message as
/// separate fields rather than one formatted string.
pub struct DiagnosticView {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub range: Range,
    pub message: String,
}

impl Diagnostic {
    pub fn to_view(&self, line_index: &LineIndex, source: &str) -> DiagnosticView {
        let (start_line, start_column) = line_index.line_col_utf8(self.start, source);
        let (end_line, end_column) = line_index.line_col_utf8(self.end, source);
        DiagnosticView {
            code: self.code,
            severity: self.severity,
            range: Range {
                start: Position {
                    line: start_line,
                    column: start_column,
                },
                end: Position {
                    line: end_line,
                    column: end_column,
                },
            },
            message: self.message.clone(),
        }
    }
}
