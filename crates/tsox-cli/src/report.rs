//! `--report`, `--report-issue`, `--report-submit`: turning the places where tsox gave up into an
//! issue the maintainers can act on.
//!
//! A "give-up" is a `TSR9xxx` warning: tsox met something it does not check yet (an expression kind,
//! an annotation it cannot resolve, a limit it hit) and went on with a lenient answer instead of
//! guessing. Those are exactly the gaps between tsox and tsc, so they are what is worth reporting.
//!
//! Nothing leaves the machine unless the person runs `--report-submit` and confirms, or opens the
//! link `--report-issue` prints. The text sent never holds file names, paths or source code, and
//! the quoted names inside messages (`'Foo'`) are replaced with `'_'` unless `--report-names` is
//! given. `--report <file>` writes the full local version, with relative file names and lines.

use std::collections::BTreeMap;

pub const DEFAULT_REPO: &str = "tsoxide/tsox";

/// One place where tsox gave up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GaveUp {
    pub file: String,
    pub line: u32,
    pub code: String,
    pub message: String,
}

pub struct Context {
    pub version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub flags: String,
    pub files: usize,
    pub errors: usize,
    pub warnings: usize,
}

/// Whether a diagnostic code is one of the "not checked here" markers.
pub fn is_give_up(code: &str) -> bool {
    code.starts_with("TSR9")
}

/// `'Foo' is not ...` becomes `'_' is not ...`: what is quoted in a message is a name from the
/// person's code, and the maintainers only need the shape of the message.
pub fn redact(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut inside = false;
    for c in message.chars() {
        match (c, inside) {
            ('\'', false) => {
                inside = true;
                out.push_str("'_");
            }
            ('\'', true) => {
                inside = false;
                out.push('\'');
            }
            (_, true) => {}
            (_, false) => out.push(c),
        }
    }
    if inside {
        out.push('\'');
    }
    out
}

/// A stable short id for the set of codes: the same gaps give the same id, so a maintainer can
/// search the issues for it before opening another.
pub fn fingerprint(codes: &[String]) -> String {
    let mut sorted: Vec<&String> = codes.iter().collect();
    sorted.sort();
    sorted.dedup();
    let mut hash: u64 = 0xcbf29ce484222325;
    for code in sorted {
        for byte in code.bytes().chain(std::iter::once(b'|')) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("{:08x}", (hash >> 32) as u32 ^ hash as u32)
}

struct Group<'a> {
    count: usize,
    example: &'a GaveUp,
}

fn groups(gave_up: &[GaveUp]) -> BTreeMap<&str, Group<'_>> {
    let mut by_code: BTreeMap<&str, Group<'_>> = BTreeMap::new();
    for item in gave_up {
        by_code
            .entry(item.code.as_str())
            .and_modify(|g| g.count += 1)
            .or_insert(Group {
                count: 1,
                example: item,
            });
    }
    by_code
}

fn header(ctx: &Context, gave_up: &[GaveUp]) -> String {
    let codes: Vec<String> = gave_up.iter().map(|g| g.code.clone()).collect();
    format!(
        "tsox {} on {}-{}\nchecked {} file(s): {} error(s), {} warning(s); {} place(s) tsox did not check\ncommand flags: {}\nreport id: tsox-report:{}\n",
        ctx.version,
        ctx.os,
        ctx.arch,
        ctx.files,
        ctx.errors,
        ctx.warnings,
        gave_up.len(),
        if ctx.flags.is_empty() {
            "(none)"
        } else {
            &ctx.flags
        },
        fingerprint(&codes)
    )
}

/// The full local report: relative file names and lines, nothing redacted. Written to a file
/// the person keeps; never sent by tsox.
pub fn local_markdown(ctx: &Context, gave_up: &[GaveUp], root: &std::path::Path) -> String {
    let mut out = String::from("# tsox: places it did not check\n\n```\n");
    out += &header(ctx, gave_up);
    out += "```\n\n| Code | Count | Example message |\n| --- | ---: | --- |\n";
    for (code, group) in groups(gave_up) {
        out += &format!(
            "| {code} | {} | {} |\n",
            group.count,
            group.example.message.replace('|', "\\|")
        );
    }
    out += "\n## Sites\n\n";
    for item in gave_up {
        let path = std::path::Path::new(&item.file);
        let shown = path.strip_prefix(root).unwrap_or(path);
        out += &format!(
            "- `{}:{}` {} {}\n",
            shown.display(),
            item.line,
            item.code,
            item.message
        );
    }
    out
}

/// What goes into an issue: codes, counts and messages. No file names, no lines, no source.
pub fn issue_body(ctx: &Context, gave_up: &[GaveUp], keep_names: bool) -> String {
    let mut out = String::from(
        "tsox reported that it does not check the places below. Nothing here is source code or a path.\n\n```\n",
    );
    out += &header(ctx, gave_up);
    out += "```\n\n| Code | Count | Message |\n| --- | ---: | --- |\n";
    for (code, group) in groups(gave_up) {
        let message = if keep_names {
            group.example.message.clone()
        } else {
            redact(&group.example.message)
        };
        out += &format!(
            "| {code} | {} | {} |\n",
            group.count,
            message.replace('|', "\\|").replace('\n', " ")
        );
    }
    out += "\nWhat the code looked like (optional, written by the reporter):\n\n";
    out
}

pub fn issue_title(gave_up: &[GaveUp]) -> String {
    let groups = groups(gave_up);
    let codes: Vec<&str> = groups.keys().copied().take(4).collect();
    let more = if groups.len() > 4 { ", ..." } else { "" };
    format!(
        "[unsupported] {}{} ({} places)",
        codes.join(", "),
        more,
        gave_up.len()
    )
}

/// Percent-encoding for a query value: everything but the unreserved characters.
pub fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 2);
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The link that opens GitHub's new-issue form with the title and body filled in. GitHub takes
/// roughly 8 KB of URL; the body is cut at a line end to stay under that.
pub fn issue_url(repo: &str, title: &str, body: &str) -> String {
    let budget = 7000usize.saturating_sub(encode(title).len() + repo.len() + 60);
    let mut kept = String::new();
    for line in body.lines() {
        let next = format!("{kept}{line}\n");
        if encode(&next).len() > budget {
            kept.push_str("\n(truncated to fit a link; the full text is in the report file)\n");
            break;
        }
        kept = next;
    }
    format!(
        "https://github.com/{repo}/issues/new?title={}&body={}",
        encode(title),
        encode(&kept)
    )
}

/// Best effort: ask the desktop to open the link. A failure only means the person copies it.
pub fn open_in_browser(url: &str) {
    use std::process::{Command, Stdio};
    let attempt = if cfg!(target_os = "windows") {
        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    } else if cfg!(target_os = "macos") {
        Command::new("open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    } else {
        Command::new("xdg-open")
            .arg(url)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    };
    let _ = attempt;
}

/// Creates the issue with the GitHub CLI after the person has seen the exact text and said yes.
/// Returns the issue's address, or why it could not.
pub fn submit_with_gh(repo: &str, title: &str, body: &str) -> Result<String, String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("gh")
        .args([
            "issue",
            "create",
            "--repo",
            repo,
            "--title",
            title,
            "--body-file",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("the GitHub CLI (gh) is not available: {e}"))?;
    child
        .stdin
        .take()
        .ok_or("could not talk to gh")?
        .write_all(body.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(code: &str, message: &str) -> GaveUp {
        GaveUp {
            file: "/home/me/secret/app.ts".into(),
            line: 7,
            code: code.into(),
            message: message.into(),
        }
    }
    fn ctx() -> Context {
        Context {
            version: "0.0.0",
            os: "linux",
            arch: "x86_64",
            flags: String::new(),
            files: 3,
            errors: 0,
            warnings: 2,
        }
    }

    #[test]
    fn quoted_names_are_redacted() {
        assert_eq!(redact("'Foo' is not 'Bar<Baz>'"), "'_' is not '_'");
        assert_eq!(redact("no names here"), "no names here");
        assert_eq!(redact("odd 'quote"), "odd '_'");
    }

    #[test]
    fn the_issue_text_has_no_path_line_or_name() {
        let gave_up = vec![
            item("TSR9012", "annotation 'SecretType' could not be resolved"),
            item("TSR9012", "annotation 'Other' could not be resolved"),
        ];
        let body = issue_body(&ctx(), &gave_up, false);
        assert!(!body.contains("secret"));
        assert!(!body.contains("app.ts"));
        assert!(!body.contains("SecretType"));
        assert!(body.contains("| TSR9012 | 2 |"));
        assert!(issue_body(&ctx(), &gave_up, true).contains("SecretType"));
    }

    #[test]
    fn the_local_report_keeps_relative_sites() {
        let gave_up = vec![item("TSR9001", "call kind")];
        let text = local_markdown(&ctx(), &gave_up, std::path::Path::new("/home/me/secret"));
        assert!(text.contains("`app.ts:7` TSR9001 call kind"));
    }

    #[test]
    fn the_id_is_stable_and_order_free() {
        let a = fingerprint(&["TSR9002".into(), "TSR9001".into()]);
        let b = fingerprint(&["TSR9001".into(), "TSR9002".into(), "TSR9001".into()]);
        assert_eq!(a, b);
        assert_ne!(a, fingerprint(&["TSR9001".into()]));
    }

    #[test]
    fn encoding_and_the_link_stay_in_bounds() {
        assert_eq!(encode("a b&c"), "a%20b%26c");
        let long: String = (0..2000)
            .map(|i| format!("line {i} with some text\n"))
            .collect();
        let url = issue_url(DEFAULT_REPO, "t", &long);
        assert!(url.len() < 8000, "{}", url.len());
        assert!(url.starts_with("https://github.com/tsoxide/tsox/issues/new?title=t&body="));
    }

    #[test]
    fn only_the_not_checked_codes_count() {
        assert!(is_give_up("TSR9012"));
        assert!(!is_give_up("TSR1003"));
    }
}
