use crate::diagnostic_codes::DiagnosticCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "wasm", derive(serde::Serialize, serde::Deserialize))]
pub enum Severity {
    Error,
    Warning,
}
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub message: String,
    pub file_name: String,

    // Byte offsets into the source, not line and column. Converting to line and
    // column needs the LineIndex built during parsing, which is not always
    // available at the point a diagnostic gets constructed, so the raw offsets are
    // kept here and converted lazily by whichever formatter actually needs them.
    pub start: u32,
    pub end: u32,
}

impl Diagnostic {
    pub fn format_with_position(
        &self,
        line_index: &crate::line_index::LineIndex,
        source: &str,
    ) -> String {
        self.format_with_code(line_index, source, false)
    }

    /// The same line with the code written as tsc's (`TS2322`) when `tsc_codes` is set
    /// and the diagnostic has one; a diagnostic with none (a "not yet checked" marker)
    /// keeps its own code. A message that is a chain of reasons continues on the
    /// following lines, indented, exactly as tsc prints it.
    pub fn format_with_code(
        &self,
        line_index: &crate::line_index::LineIndex,
        source: &str,
        tsc_codes: bool,
    ) -> String {
        let view = self.to_view(line_index, source);
        let code = if tsc_codes { self.code.tsc_str() } else { None }
            .unwrap_or_else(|| self.code.to_string());
        format!(
            "{}:{}:{}: {}: {} {}",
            self.file_name,
            view.range.start.line,
            view.range.start.column,
            match self.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
            },
            code,
            view.message
        )
    }
}
