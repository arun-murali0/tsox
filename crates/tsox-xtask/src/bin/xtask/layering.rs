//! Fail when a workspace crate depends on a crate at its own layer or above (docs/architecture/workspace.md).
//! Dev-dependencies count too, so a test cannot quietly pull a layer upwards.

use std::collections::{BTreeSet, HashMap};
use std::process::Command;

/// Lowest first. A crate may depend on any crate in an earlier layer, never on one in its own layer
/// or a later one. The front ends share the top layer, so none depends on another.
const LAYERS: &[&[&str]] = &[
    &["tsox-base"],
    &["tsox-types"],
    &["tsox-model"],
    &["tsox-checker"],
    &["tsox-project"],
    &["tsox-db"],
    &["tsox"],
    &["tsox-ir"],
    &[
        "tsox-cli",
        "tsox-emit",
        "tsox-lsp",
        "tsox-wasm",
        "tsox-xtask",
    ],
];

pub fn run() -> u8 {
    let rank: HashMap<&str, usize> = LAYERS
        .iter()
        .enumerate()
        .flat_map(|(r, l)| l.iter().map(move |n| (*n, r)))
        .collect();
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .current_dir(crate::root())
        .output()
        .expect("run cargo metadata");
    if !out.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        return 1;
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).expect("cargo metadata json");
    let packages = meta["packages"].as_array().expect("packages");
    let names: BTreeSet<&str> = packages.iter().filter_map(|p| p["name"].as_str()).collect();
    let mut problems = Vec::new();

    let unknown: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| !rank.contains_key(n))
        .collect();
    if !unknown.is_empty() {
        problems.push(format!(
            "crate(s) {} have no layer; add them to LAYERS in crates/tsox-xtask/src/bin/xtask/layering.rs and to docs/architecture/workspace.md",
            unknown.join(", ")
        ));
    }
    let mut missing: Vec<&str> = rank
        .keys()
        .copied()
        .filter(|n| !names.contains(n))
        .collect();
    missing.sort();
    if !missing.is_empty() {
        problems.push(format!(
            "layer(s) {} listed here but not in the workspace",
            missing.join(", ")
        ));
    }
    for pkg in packages {
        let name = pkg["name"].as_str().unwrap_or("");
        let Some(&mine) = rank.get(name) else {
            continue;
        };
        for dep in pkg["dependencies"].as_array().into_iter().flatten() {
            let dep_name = dep["name"].as_str().unwrap_or("");
            let Some(&theirs) = rank.get(dep_name) else {
                continue;
            };
            if theirs >= mine {
                let kind = dep["kind"].as_str().unwrap_or("normal");
                problems.push(format!(
                    "{name} ({kind}) depends on {dep_name}, which is not below it"
                ));
            }
        }
    }
    problems.extend(oxc_outside_bridge());
    for p in &problems {
        eprintln!("layering: {p}");
    }
    if problems.is_empty() {
        let chain: Vec<String> = LAYERS.iter().map(|l| l.join(" | ")).collect();
        println!("layering: ok ({})", chain.join(" < "));
        0
    } else {
        1
    }
}

/// The checker reads the Oxc AST only in `bridge/`; what the rest of the crate sees is what
/// `bridge` returns (docs/architecture/flow.md). Any other file of the crate that names an
/// `oxc_` crate breaks that.
fn oxc_outside_bridge() -> Vec<String> {
    fn walk(dir: &std::path::Path, bridge: &std::path::Path, found: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path != bridge {
                    walk(&path, bridge, found);
                }
            } else if path.extension().is_some_and(|e| e == "rs")
                && std::fs::read_to_string(&path).is_ok_and(|text| {
                    text.lines()
                        .any(|l| l.contains("oxc_") && !l.trim_start().starts_with("//"))
                })
            {
                found.push(format!(
                    "{} names an Oxc crate outside tsox-checker/src/bridge/",
                    path.strip_prefix(crate::root()).unwrap_or(&path).display()
                ));
            }
        }
    }
    let src = crate::root().join("crates/tsox-checker/src");
    let mut found = Vec::new();
    walk(&src, &src.join("bridge"), &mut found);
    found
}
