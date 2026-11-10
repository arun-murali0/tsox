//! The shape of a file as ranges: what can be folded, and the nested ranges a selection grows
//! through. Both read the syntax tree only.

use oxc_ast::AstKind;
use oxc_ast::ast::{CommentKind, ModuleDeclaration};
use oxc_span::GetSpan;

use crate::navigate::with_semantic;
use crate::text::LineMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Comment,
    Imports,
    Region,
    Code,
}

/// A run of lines that can be folded away. `end_line` is the last line hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: FoldKind,
}

fn dummy_path(file_name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(file_name)
}

/// The foldable ranges of a file: bodies and literals that span lines, runs of imports, block
/// comments and runs of line comments, and `// #region` pairs.
pub fn folding(text: &str, file_name: &str) -> Vec<Fold> {
    let map = LineMap::new(text);
    let line_of = |offset: u32| map.position(text, offset as usize).line;
    let lines: Vec<&str> = text.split('\n').collect();
    let mut found: Vec<Fold> = Vec::new();

    let mut push = |start: u32, end: u32, kind: FoldKind, bracketed: bool| {
        let start_line = line_of(start);
        let mut end_line = line_of(end);
        // A block folds down to the line before its closing bracket, which stays visible.
        if bracketed
            && end_line > start_line
            && lines.get(end_line as usize).is_some_and(|line| {
                matches!(line.trim_start().chars().next(), Some('}' | ']' | ')'))
            })
        {
            end_line -= 1;
        }
        if end_line > start_line {
            found.push(Fold {
                start_line,
                end_line,
                kind,
            });
        }
    };

    let analysed = with_semantic(&dummy_path(file_name), text, |program, semantic| {
        for node in semantic.nodes().iter() {
            let span = match node.kind() {
                AstKind::BlockStatement(n) => n.span,
                AstKind::FunctionBody(n) => n.span,
                AstKind::ClassBody(n) => n.span,
                AstKind::TSInterfaceBody(n) => n.span,
                AstKind::TSEnumBody(n) => n.span,
                AstKind::TSModuleBlock(n) => n.span,
                AstKind::TSTypeLiteral(n) => n.span,
                AstKind::ObjectExpression(n) => n.span,
                AstKind::ArrayExpression(n) => n.span,
                AstKind::SwitchStatement(n) => n.span,
                AstKind::TemplateLiteral(n) => n.span,
                _ => continue,
            };
            push(span.start, span.end, FoldKind::Code, true);
        }

        // Runs of imports.
        let mut run: Option<(u32, u32)> = None;
        for statement in &program.body {
            let is_import = matches!(
                statement.as_module_declaration(),
                Some(ModuleDeclaration::ImportDeclaration(_))
            );
            let span = statement.span();
            match (is_import, run) {
                (true, None) => run = Some((span.start, span.end)),
                (true, Some((start, _))) => run = Some((start, span.end)),
                (false, Some((start, end))) => {
                    push(start, end, FoldKind::Imports, false);
                    run = None;
                }
                (false, None) => {}
            }
        }
        if let Some((start, end)) = run {
            push(start, end, FoldKind::Imports, false);
        }

        // Comments: block comments, runs of line comments, regions.
        let mut line_run: Option<(u32, u32, u32)> = None; // first start, last end, last line
        let mut regions: Vec<u32> = Vec::new();
        let flush = |run: &mut Option<(u32, u32, u32)>,
                     push: &mut dyn FnMut(u32, u32, FoldKind, bool)| {
            if let Some((start, end, _)) = run.take() {
                push(start, end, FoldKind::Comment, false);
            }
        };
        for comment in &program.comments {
            let body = text
                .get(comment.span.start as usize..comment.span.end as usize)
                .unwrap_or("");
            match comment.kind {
                CommentKind::Line => {
                    let marker = body.trim_start_matches('/').trim();
                    if marker.starts_with("#region") || marker.starts_with("region") {
                        regions.push(comment.span.start);
                    } else if marker.starts_with("#endregion") || marker.starts_with("endregion") {
                        if let Some(start) = regions.pop() {
                            push(start, comment.span.end, FoldKind::Region, false);
                        }
                    }
                    let line = line_of(comment.span.start);
                    match line_run {
                        Some((start, _, last)) if last + 1 == line => {
                            line_run = Some((start, comment.span.end, line));
                        }
                        _ => {
                            flush(&mut line_run, &mut push);
                            line_run = Some((comment.span.start, comment.span.end, line));
                        }
                    }
                }
                _ => {
                    flush(&mut line_run, &mut push);
                    push(
                        comment.span.start,
                        comment.span.end,
                        FoldKind::Comment,
                        false,
                    );
                }
            }
        }
        flush(&mut line_run, &mut push);
    });
    if analysed.is_none() {
        return Vec::new();
    }

    // Of ranges that start on the same line, the longest wins.
    found.sort_by_key(|fold| (fold.start_line, std::cmp::Reverse(fold.end_line)));
    found.dedup_by_key(|fold| fold.start_line);
    found
}

/// The ranges that contain `offset`, smallest first, ending with the whole file: what each
/// "expand selection" steps through.
pub fn selection_chain(text: &str, file_name: &str, offset: usize) -> Vec<(u32, u32)> {
    let Ok(offset) = u32::try_from(offset) else {
        return vec![(0, text.len() as u32)];
    };
    let mut spans: Vec<(u32, u32)> = with_semantic(&dummy_path(file_name), text, |_, semantic| {
        semantic
            .nodes()
            .iter()
            .map(|node| node.kind().span())
            .filter(|span| span.start <= offset && offset <= span.end && span.end > span.start)
            .map(|span| (span.start, span.end))
            .collect()
    })
    .unwrap_or_default();
    spans.sort_by_key(|(start, end)| (end - start, *start));
    spans.dedup();
    // Only ranges that grow: each holds the one before it.
    let mut chain: Vec<(u32, u32)> = Vec::new();
    for span in spans {
        if chain
            .last()
            .is_none_or(|last| span.0 <= last.0 && span.1 >= last.1 && span != *last)
        {
            chain.push(span);
        }
    }
    let whole = (0, text.len() as u32);
    if chain.last() != Some(&whole) {
        chain.push(whole);
    }
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folds(text: &str) -> Vec<(u32, u32, FoldKind)> {
        folding(text, "a.ts")
            .into_iter()
            .map(|f| (f.start_line, f.end_line, f.kind))
            .collect()
    }

    #[test]
    fn a_function_body_folds_to_the_line_before_its_closing_brace() {
        let text = "function f() {\n  one();\n  two();\n}\n";
        assert_eq!(folds(text), vec![(0, 2, FoldKind::Code)]);
    }

    #[test]
    fn a_body_on_one_line_does_not_fold() {
        assert_eq!(
            folds("function f() { return 1; }\nconst o = { a: 1 };\n"),
            vec![]
        );
    }

    #[test]
    fn nested_bodies_fold_separately_and_one_start_line_has_one_fold() {
        let text = "class C {\n  m() {\n    if (x) {\n      y();\n    }\n  }\n}\nconst o = {\n  a: [\n    1,\n  ],\n};\n";
        assert_eq!(
            folds(text),
            vec![
                (0, 5, FoldKind::Code),
                (1, 4, FoldKind::Code),
                (2, 3, FoldKind::Code),
                (7, 10, FoldKind::Code),
                (8, 9, FoldKind::Code),
            ]
        );
    }

    #[test]
    fn interfaces_enums_namespaces_and_type_literals_fold() {
        let text = "interface I {\n  a: number;\n}\nenum E {\n  A,\n}\nnamespace N {\n  const x = 1;\n}\ntype T = {\n  a: 1;\n};\n";
        let kinds: Vec<u32> = folds(text).into_iter().map(|f| f.0).collect();
        assert_eq!(kinds, vec![0, 3, 6, 9]);
    }

    #[test]
    fn a_run_of_imports_folds_as_imports() {
        let text = "import a from 'a';\nimport b from 'b';\nimport c from 'c';\nconst x = 1;\n";
        assert_eq!(folds(text), vec![(0, 2, FoldKind::Imports)]);
        assert_eq!(folds("import a from 'a';\nconst x = 1;\n"), vec![]);
    }

    #[test]
    fn comments_fold_as_blocks_and_as_runs_of_lines() {
        let text = "/**\n * docs\n */\nfunction f() {}\n// one\n// two\n// three\nconst x = 1;\n// alone\n";
        assert_eq!(
            folds(text),
            vec![(0, 2, FoldKind::Comment), (4, 6, FoldKind::Comment)]
        );
    }

    #[test]
    fn region_markers_fold() {
        let text = "// #region helpers\nconst a = 1;\nconst b = 2;\n// #endregion\n";
        let found = folds(text);
        assert!(found.contains(&(0, 3, FoldKind::Region)), "{found:?}");
    }

    #[test]
    fn a_half_typed_file_still_folds_what_came_before() {
        let text = "function done() {\n  a();\n}\nclass Half {\n  m() {\n";
        assert!(folds(text).contains(&(0, 1, FoldKind::Code)));
    }

    #[test]
    fn a_selection_grows_from_the_name_to_the_file() {
        let text = "function f(a: number) { return a + 1; }\n";
        let at = text.find("a + 1").unwrap();
        let chain = selection_chain(text, "a.ts", at);
        let shown: Vec<&str> = chain
            .iter()
            .map(|(s, e)| &text[*s as usize..*e as usize])
            .collect();
        assert_eq!(shown[0], "a");
        assert!(shown.contains(&"a + 1"));
        assert!(shown.contains(&"return a + 1;"));
        assert_eq!(*shown.last().unwrap(), text);
        // Each range holds the one before it, and none repeats.
        for pair in chain.windows(2) {
            assert!(pair[1].0 <= pair[0].0 && pair[1].1 >= pair[0].1 && pair[0] != pair[1]);
        }
    }

    #[test]
    fn a_selection_in_an_empty_file_is_the_whole_file() {
        assert_eq!(selection_chain("", "a.ts", 0), vec![(0, 0)]);
    }
}
