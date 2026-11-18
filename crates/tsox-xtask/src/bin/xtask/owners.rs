//! Owner map of every `DiagnosticCode`: the message function that builds it and the files that call
//! that function. Writes docs/architecture/diagnostic-owners.md, or with `--check` compares the file
//! on disk with what the code says and fails when a code has no message function or the file is stale.

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// Codes that exist but that nothing raises yet. Raising one is a first task; remove it from here
/// when it is raised. A new code that nothing raises fails `--check` unless it is listed.
const KNOWN_UNRAISED: &[&str] = &["RedeclareExportedVariable"];

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `text.split(delim)[1]`: what lies between the first and the second occurrence.
fn second_piece<'a>(text: &'a str, delim: &str) -> &'a str {
    let after = text.split_once(delim).map(|(_, rest)| rest).unwrap_or("");
    after.split(delim).next().unwrap_or("")
}

pub fn run(check: bool) -> u8 {
    let root = crate::root();
    let codes_path = root.join("crates/tsox-base/src/diagnostic_codes.rs");
    let messages_path = root.join("crates/tsox-types/src/diagnostic_messages.rs");
    let out_path = root.join("docs/architecture/diagnostic-owners.md");

    let codes_src = fs::read_to_string(&codes_path).expect("diagnostic_codes.rs");
    let ids: HashMap<String, String> = Regex::new(r#"(?m)^\s+(\w+) => "(TSR\d+)""#)
        .unwrap()
        .captures_iter(&codes_src)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    let enum_body = second_piece(&codes_src, "pub enum DiagnosticCode {");
    let enum_body = enum_body.split("\n}\n").next().unwrap_or("");
    let variants: Vec<String> = Regex::new(r"(?m)^    (\w+),$")
        .unwrap()
        .captures_iter(enum_body)
        .map(|c| c[1].to_string())
        .collect();

    // The tsc code of each variant, from the `tsc_code` match: arms are split at a comma that follows a digit.
    let mut tsc: HashMap<String, String> = HashMap::new();
    let body = second_piece(&codes_src, "pub fn tsc_code");
    let body = body.split("return None").next().unwrap_or("");
    let upper = Regex::new(r"[A-Z]\w+").unwrap();
    let mut arms = Vec::new();
    let bytes = body.as_bytes();
    let mut from = 0;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b',' && i > 0 && bytes[i - 1].is_ascii_digit() {
            arms.push(&body[from..i]);
            from = i + 1;
        }
    }
    arms.push(&body[from..]);
    for arm in arms {
        let Some((names, code)) = arm.rsplit_once("=>") else {
            continue;
        };
        let code = code.trim();
        if !code.is_empty() && code.chars().all(|c| c.is_ascii_digit()) {
            let tail = names.rsplit('{').next().unwrap_or("");
            for m in upper.find_iter(tail) {
                tsc.insert(m.as_str().to_string(), code.to_string());
            }
        }
    }

    // Message functions and the variants each one builds.
    let msg_src = fs::read_to_string(&messages_path).expect("diagnostic_messages.rs");
    let pub_fn = Regex::new(r"pub fn (\w+)\b").unwrap();
    let boundary = Regex::new(r"\n    pub fn |\n\}\n").unwrap();
    let variant_ref = Regex::new(r"DiagnosticCode::(\w+)").unwrap();
    let mut fn_variants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut variant_fns: HashMap<String, Vec<String>> = HashMap::new();
    for m in pub_fn.captures_iter(&msg_src) {
        let whole = m.get(0).unwrap();
        let rest = &msg_src[whole.end()..];
        let end = boundary.find(rest).map(|b| b.start()).unwrap_or(rest.len());
        let chunk = &rest[..end];
        let set: BTreeSet<String> = variant_ref
            .captures_iter(chunk)
            .map(|c| c[1].to_string())
            .collect();
        for v in set {
            fn_variants
                .entry(m[1].to_string())
                .or_default()
                .insert(v.clone());
            variant_fns.entry(v).or_default().push(m[1].to_string());
        }
    }

    // Call sites by file, ignoring tests and the message file itself.
    let mut files = Vec::new();
    rs_files(&root.join("crates"), &mut files);
    files.retain(|p| {
        let s = p.to_string_lossy();
        !s.contains("/tests/") && !s.contains("/crates/tsox-xtask/") && *p != messages_path
    });
    files.sort();
    let texts: Vec<(PathBuf, String)> = files
        .into_iter()
        .filter_map(|p| fs::read_to_string(&p).ok().map(|t| (p, t)))
        .collect();
    let area = |p: &Path| -> String {
        let rel = p.strip_prefix(root.join("crates")).unwrap();
        let parts: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let rest = &parts[2.min(parts.len())..];
        let tail = if rest.len() > 1 {
            rest[..2].join("/")
        } else {
            rest.first().cloned().unwrap_or_default()
        };
        format!("{}/{}", parts[0], tail).replace(".rs", "")
    };
    let call = Regex::new(r"\b(\w+)\s*\(").unwrap();
    let ident = Regex::new(r"\b([A-Z]\w+)\b").unwrap();
    let called: Vec<(&PathBuf, BTreeSet<&str>)> = texts
        .iter()
        .map(|(p, t)| {
            (
                p,
                call.captures_iter(t)
                    .map(|c| c.get(1).unwrap().as_str())
                    .collect(),
            )
        })
        .collect();
    let callers: HashMap<&String, BTreeSet<String>> = fn_variants
        .keys()
        .map(|f| {
            (
                f,
                called
                    .iter()
                    .filter(|(_, n)| n.contains(f.as_str()))
                    .map(|(p, _)| area(p))
                    .collect(),
            )
        })
        .collect();
    let idents: Vec<(&PathBuf, BTreeSet<&str>)> = texts
        .iter()
        .filter(|(p, _)| p != &codes_path)
        .map(|(p, t)| {
            (
                p,
                ident
                    .captures_iter(t)
                    .map(|c| c.get(1).unwrap().as_str())
                    .collect(),
            )
        })
        .collect();

    let mut errors = Vec::new();
    // (id, variant, tsc, message functions, raised from)
    let mut rows: Vec<(String, String, String, String, String)> = Vec::new();
    for v in &variants {
        let fns = variant_fns.get(v).cloned().unwrap_or_default();
        let mut owners: BTreeSet<String> = BTreeSet::new();
        for f in &fns {
            if let Some(a) = callers.get(f) {
                owners.extend(a.iter().cloned());
            }
        }
        owners.extend(
            idents
                .iter()
                .filter(|(_, n)| n.contains(v.as_str()))
                .map(|(p, _)| area(p)),
        );
        if fns.is_empty() && owners.is_empty() && !KNOWN_UNRAISED.contains(&v.as_str()) {
            errors.push(format!(
                "{v}: nothing builds or names it (raise it, or list it in KNOWN_UNRAISED)"
            ));
        }
        let quote = |items: Vec<String>| {
            if items.is_empty() {
                "-".to_string()
            } else {
                items
                    .iter()
                    .map(|i| format!("`{i}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        };
        rows.push((
            ids.get(v).cloned().unwrap_or_else(|| "?".into()),
            v.clone(),
            tsc.get(v)
                .map(|c| format!("TS{c}"))
                .unwrap_or_else(|| "-".into()),
            quote(fns),
            if owners.is_empty() {
                "(none found)".into()
            } else {
                quote(owners.into_iter().collect())
            },
        ));
    }
    rows.sort();

    let mut lines: Vec<String> = vec![
        "# Diagnostic owners".into(),
        String::new(),
        "Generated by `cargo xtask owners`; do not edit. Each row is one `DiagnosticCode`: its tsox id, the tsc code it maps to,".into(),
        "the message function that builds it, and the code areas that call that function (the places to read or change when the diagnostic is wrong).".into(),
        String::new(),
        "| tsox id | Variant | tsc | Message function | Raised from |".into(),
        "| --- | --- | --- | --- | --- |".into(),
    ];
    for (a, b, c, d, e) in &rows {
        lines.push(format!("| {a} | `{b}` | {c} | {d} | {e} |"));
    }
    let backlog: Vec<_> = rows
        .iter()
        .filter(|r| {
            r.0.starts_with("TSR90") && !r.0.starts_with("TSR910") && !r.0.starts_with("TSR920")
        })
        .collect();
    lines.extend([
        String::new(),
        "## Backlog: conditions not yet checked (9000s)".into(),
        String::new(),
        "Each is a place where tsox gives up with a warning instead of checking. Pick one, add a fixture from tsc 6.0.3, and replace the warning with the real check.".into(),
        String::new(),
        "| tsox id | Variant | Raised from |".into(),
        "| --- | --- | --- |".into(),
    ]);
    for (a, b, _, _, e) in &backlog {
        lines.push(format!("| {a} | `{b}` | {e} |"));
    }
    lines.extend([
        String::new(),
        "## Defined but never raised".into(),
        String::new(),
        "The code and its message exist, but no check calls them. Each is a gap against tsc and a first task.".into(),
        String::new(),
        "| tsox id | Variant | tsc | Message function |".into(),
        "| --- | --- | --- | --- |".into(),
    ]);
    for (a, b, c, d, _) in rows.iter().filter(|r| r.4 == "(none found)") {
        lines.push(format!("| {a} | `{b}` | {c} | {d} |"));
    }
    let text = lines.join("\n") + "\n";

    if check {
        if !errors.is_empty() {
            println!("{}", errors.join("\n"));
            return 1;
        }
        if fs::read_to_string(&out_path).ok().as_deref() != Some(text.as_str()) {
            println!("docs/architecture/diagnostic-owners.md is stale: run cargo xtask owners");
            return 1;
        }
        println!("diagnostic owners: ok ({} codes)", rows.len());
    } else {
        fs::write(&out_path, &text).expect("write diagnostic-owners.md");
        println!(
            "wrote docs/architecture/diagnostic-owners.md ({} codes, {} backlog)",
            rows.len(),
            backlog.len()
        );
        if !errors.is_empty() {
            println!("{}", errors.join("\n"));
        }
    }
    0
}
