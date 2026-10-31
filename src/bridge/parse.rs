use oxc_allocator::Allocator;
use oxc_ast::ast::Program;
use oxc_parser::Parser;
use oxc_semantic::{Semantic, SemanticBuilder};
use oxc_span::SourceType;

use crate::error::CheckerError;

// The parser's wording for the three tuple shape rules (TS1257, TS1265, TS1266). They are the
// parser's own text for tsc's, so they are matched whole.
const TUPLE_SHAPE_ERRORS: [&str; 3] = [
    "A required element cannot follow an optional element.",
    "A rest element cannot follow another rest element.",
    "An optional element cannot follow a rest element.",
];

#[tracing::instrument(skip(allocator, source), fields(file_name, source_len = source.len()))]
pub fn parse<'a>(
    allocator: &'a Allocator,
    source: &'a str,
    file_name: &str,
) -> Result<Program<'a>, CheckerError> {
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

    let result = Parser::new(allocator, source, source_type).parse();

    if result.diagnostics.has_errors() {
        let messages: Vec<String> = result
            .diagnostics
            .errors()
            .map(ToString::to_string)
            .filter(|message| !TUPLE_SHAPE_ERRORS.contains(&message.as_str()))
            .collect();
        // Problem: `type T = [string?, number]` made the parser report a syntax error, and a
        // syntax error refuses the whole file, so one misplaced `?` stopped every other
        // check in it, with no position and no code. tsc reports these three tuple rules as
        // ordinary errors at the element and keeps checking.
        // Picked: the parser still builds the whole tree for them, so a file whose only
        // errors are these three is parsed normally and tuple_grammar.rs reports each one
        // at its element, under its own code. Any other error still refuses the file.
        if !messages.is_empty() {
            tracing::warn!(error_count = messages.len(), "parse completed with errors");
            return Err(CheckerError::Parse(messages.join("; ")));
        }
        tracing::debug!("parse reported only tuple shape errors; continuing");
    }

    tracing::debug!(
        statement_count = result.program.body.len(),
        "parsed successfully"
    );
    Ok(result.program)
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
