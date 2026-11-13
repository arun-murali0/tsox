use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_parser::Parser;
use oxc_semantic::{Semantic, SemanticBuilder};
use oxc_span::SourceType;

use crate::error::CheckerError;

// The parser's wording for the rules the checker reports itself, at the offending node and
// under tsc's code: the three tuple shape rules (TS1257, TS1265, TS1266) and the two ambient
// context rules (TS1039, TS1183; see `ambient_checks`). They are the parser's own text for tsc's,
// so they are matched whole. The parser still builds the whole tree for each of them.
const TUPLE_SHAPE_ERRORS: [&str; 5] = [
    "A required element cannot follow an optional element.",
    "A rest element cannot follow another rest element.",
    "An optional element cannot follow a rest element.",
    "Initializers are not allowed in ambient contexts.",
    "An implementation cannot be declared in ambient contexts.",
];

#[tracing::instrument(skip(allocator, source), fields(file_name, source_len = source.len()))]
pub fn parse<'a>(
    allocator: &'a Allocator,
    source: &'a str,
    file_name: &str,
) -> Result<Program<'a>, CheckerError> {
    parse_with_syntax_errors(allocator, source, file_name).map(|parsed| parsed.program)
}

pub struct Parsed<'a> {
    pub program: Program<'a>,
    // The syntax errors that have a tsc code and wording (`syntax_errors`), at offsets of the
    // original source.
    pub syntax: Vec<super::syntax_errors::SyntaxDiagnostic>,
    // The parser gave up and the program is empty, so nothing about the file is known.
    pub gave_up: bool,
    // Set when the parser was handed a repaired copy of the source: the program's offsets are
    // those of the copy, and every offset reported goes back through this.
    pub offsets: Option<super::syntax_repair::OffsetMap>,
}

// One parse of one text. `first_error` is where the parser stopped, when it gave up.
struct Attempt<'a> {
    program: Program<'a>,
    syntax: Vec<super::syntax_errors::SyntaxDiagnostic>,
    messages: Vec<String>,
    gave_up: bool,
    first_error: Option<(usize, usize, String)>,
}

fn parse_attempt<'a>(
    allocator: &'a Allocator,
    source: &'a str,
    source_type: SourceType,
) -> Attempt<'a> {
    let result = Parser::new(allocator, source, source_type).parse();
    let mut syntax = Vec::new();
    let mut messages = Vec::new();
    let mut first_error = None;
    if result.diagnostics.has_errors() {
        for error in result.diagnostics.errors() {
            let Some(label) = error.labels.first() else {
                continue;
            };
            let span =
                oxc_span::Span::new(label.offset() as u32, (label.offset() + label.len()) as u32);
            if first_error.is_none() {
                first_error = Some((
                    label.offset() as usize,
                    (label.offset() + label.len()) as usize,
                    error.message.to_string(),
                ));
            }
            if let Some(found) = super::syntax_errors::translate(source, &error.message, span) {
                syntax.push(found);
            }
        }
        messages = result
            .diagnostics
            .errors()
            .map(ToString::to_string)
            .filter(|message| !TUPLE_SHAPE_ERRORS.contains(&message.as_str()))
            .collect();
    }
    // Problem: any syntax error refused the whole file, but tsc reports the syntax error and
    // goes on to check what it recovered, so every semantic error in the rest of the file went
    // missing.
    // Picked: a parse the parser recovered from (it did not give up) is checked like any other;
    // only a parse that panicked, which has no usable tree, is a failure. The three tuple shape
    // rules and the two ambient context rules are not errors here: the checker reports them.
    let gave_up = !messages.is_empty()
        && result.program.body.is_empty()
        && result.program.directives.is_empty();
    Attempt {
        program: result.program,
        syntax,
        messages,
        gave_up,
        first_error,
    }
}

// The parse and the syntax errors in it that have a tsc code and wording (`syntax_errors`).
//
// Problem: a parse that gives up on one construct leaves no tree, so tsox checked nothing while
// tsc reports the syntax error and checks the rest. 756 corpus files were lost this way.
// Picked: for the constructs in `syntax_repair`, the source is patched into something the parser
// accepts (with tsc's error recorded for it), parsed again, and offsets go back to the original.
pub fn parse_with_syntax_errors<'a>(
    allocator: &'a Allocator,
    source: &'a str,
    file_name: &str,
) -> Result<Parsed<'a>, CheckerError> {
    // A recognized extension (.ts, .tsx, and so on) picks the right dialect
    // automatically. An unrecognized or missing extension, such as a file name
    // used only for a diagnostic label in a test, falls back to TypeScript rather
    // than plain JavaScript, since this checker has no reason to be given a name
    // it cannot map unless the source is meant to include TypeScript syntax.
    let source_type = SourceType::from_path(file_name)
        .unwrap_or_else(|_| SourceType::default().with_typescript(true));

    // The parser recurses once per level of nesting and cannot be stopped from walking into
    // a stack overflow, so a file nested past the limit is refused before it is parsed.
    let depth = crate::source_depth::max_bracket_depth(source);
    if depth > crate::stack_guard::MAX_BRACKET_DEPTH {
        return Err(CheckerError::Parse(format!(
            "brackets nest {depth} levels deep, past the limit of {}; the file was not parsed",
            crate::stack_guard::MAX_BRACKET_DEPTH
        )));
    }

    let mut attempt = parse_attempt(allocator, source, source_type);
    let mut repairs = super::syntax_repair::Repairs::new(source);
    while attempt.gave_up && repairs.len() < super::syntax_repair::MAX_REPAIRS {
        let Some((start, end, message)) = attempt.first_error.clone() else {
            break;
        };
        if !repairs.apply(start, end, &message) {
            break;
        }
        let patched: &'a str = allocator.alloc_str(repairs.text());
        attempt = parse_attempt(allocator, patched, source_type);
    }

    if attempt.gave_up {
        // Nothing is left to check, but the syntax errors tsc would report are still the
        // file's diagnostics.
        let (mut syntax, offsets) = repairs.finish(attempt.syntax);
        if !syntax.is_empty() {
            syntax.sort_by_key(|found| found.span.start);
            return Ok(Parsed {
                program: attempt.program,
                syntax,
                gave_up: true,
                offsets,
            });
        }
        tracing::warn!(
            error_count = attempt.messages.len(),
            "parse completed with errors"
        );
        return Err(CheckerError::Parse(attempt.messages.join("; ")));
    }
    if !attempt.messages.is_empty() {
        tracing::warn!(
            error_count = attempt.messages.len(),
            "parse recovered from errors; checking"
        );
    }
    tracing::debug!(
        statement_count = attempt.program.body.len(),
        "parsed successfully"
    );
    let (mut syntax, offsets) = repairs.finish(attempt.syntax);
    syntax.sort_by_key(|found| found.span.start);
    Ok(Parsed {
        program: attempt.program,
        syntax,
        gave_up: false,
        offsets,
    })
}

// Semantic analysis with control flow graph construction and the node table
// switched on. oxc leaves both off by default, since most consumers read neither
// and both cost time and memory, and leaving either flag out does not fail here:
// with_cfg(false) makes cfg() return None, and with_build_nodes(false) leaves the
// node table empty, so every node id in the graph points at nothing and the first
// lookup panics. Both are needed together, because the graph says how blocks
// connect and the node table says which block an AST node is in. The whole
// Semantic is returned, not only its Scoping, since both live on it; symbol and
// reference lookups keep working through Semantic::scoping().
pub fn analyze<'a>(program: &'a Program<'a>) -> Semantic<'a> {
    let semantic = SemanticBuilder::new()
        .with_cfg(true)
        .with_build_nodes(true)
        .build(program)
        .semantic;

    // Logged so a missing or empty graph is visible under RUST_LOG=debug. Nothing
    // consumes the graph yet, so without this a construction problem would only
    // surface later, as narrowing that quietly does not happen.
    if let Some(cfg) = semantic.cfg() {
        tracing::debug!(
            blocks = cfg.basic_blocks.len(),
            edges = cfg.graph.edge_count(),
            "control flow graph built"
        );
    }

    semantic
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pins with_cfg(true) and with_build_nodes(true). oxc's default is no graph and
    // an empty node table, and neither fails at build time: cfg() returns None, and
    // the node table's lookups panic on first use. So dropping either flag would
    // leave every later flow query with nothing to read and no error saying why. An
    // `if` with an early return cannot fit in one basic block, so a single-block
    // graph would mean the builder ran but recorded no control flow.
    #[test]
    fn analyze_builds_a_control_flow_graph_and_node_table() {
        let allocator = Allocator::default();
        let source = "function f(x: number) { if (x > 0) { return 1; } return 2; }";
        let program = parse(&allocator, source, "cfg.ts").expect("source should parse");

        let semantic = analyze(&program);
        let cfg = semantic
            .cfg()
            .expect("with_cfg(true) and the `cfg` feature must be on");

        assert!(
            cfg.basic_blocks.len() > 1,
            "expected several basic blocks, got {}",
            cfg.basic_blocks.len()
        );

        // Every node is placed in some block, so this lookup is the one a flow query
        // makes. It panics if the node table is empty, and the block it returns must
        // be one the graph actually contains.
        let nodes = semantic.nodes();
        let first = nodes.iter().next().expect("node table is empty");
        let block = nodes.cfg_id(first.id());
        assert!(
            cfg.graph.node_weight(block).is_some(),
            "a node was placed in a block the graph does not contain"
        );
    }
}
