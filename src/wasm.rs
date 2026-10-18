use wasm_bindgen::prelude::*;

use crate::line_index::LineIndex;
use crate::{TypeChecker as CoreChecker, diagnostics::Diagnostic};

// Positions here are converted with line_col_utf16, not the utf8 variant used for
// CLI output, since this struct is serialized to JavaScript, and JS strings (and
// the LSP position spec editors expect) are UTF-16 code units, not Rust's own
// Unicode scalar value count.
#[cfg_attr(feature = "wasm", derive(serde::Serialize))]
pub struct WasmDiagnostic {
    severity: crate::diagnostics::Severity,
    message: String,
    file_name: String,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

impl WasmDiagnostic {
    fn from_diagnostic(diagnostic: &Diagnostic, line_index: &LineIndex, source: &str) -> Self {
        let (start_line, start_column) = line_index.line_col_utf16(diagnostic.start, source);
        let (end_line, end_column) = line_index.line_col_utf16(diagnostic.end, source);
        Self {
            severity: diagnostic.severity,
            message: diagnostic.message.clone(),
            file_name: diagnostic.file_name.clone(),
            start_line,
            start_column,
            end_line,
            end_column,
        }
    }
}

#[wasm_bindgen(start)]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub struct TsoxChecker {
    inner: CoreChecker,
}

#[wasm_bindgen]
impl TsoxChecker {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: CoreChecker::new(),
        }
    }

    #[wasm_bindgen(js_name = checkSource)]
    pub fn check_source(&self, source: &str, file_name: &str) -> Result<JsValue, JsValue> {
        let result = self
            .inner
            .check_source(source, file_name)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        let line_index = LineIndex::new(source);
        let wasm_diagnostics: Vec<WasmDiagnostic> = result
            .diagnostics
            .iter()
            .map(|diagnostic| WasmDiagnostic::from_diagnostic(diagnostic, &line_index, source))
            .collect();

        serde_wasm_bindgen::to_value(&wasm_diagnostics)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

impl Default for TsoxChecker {
    fn default() -> Self {
        Self::new()
    }
}
