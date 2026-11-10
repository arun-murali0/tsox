//! The server as an editor sees it: the real binary, talking LSP over its pipes.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const BAD: &str = "export const n: number = 'x';\n";
const GOOD: &str = "export const n: number = 1;\n";
const WAIT: Duration = Duration::from_secs(60);

struct Client {
    child: Child,
    stdin: ChildStdin,
    messages: Receiver<Value>,
    next_id: i64,
    // Requests the server made of the client, answered as they arrive.
    server_requests: Vec<Value>,
}

impl Client {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_tsox-lsp"))
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut length = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(value) = line.strip_prefix("Content-Length: ") {
                        length = value.parse::<usize>().ok();
                    }
                }
                let mut body = vec![0; length.expect("a frame without a length")];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            messages,
            next_id: 1,
            server_requests: Vec::new(),
        }
    }

    fn write(&mut self, value: &Value) {
        let body = serde_json::to_vec(value).unwrap();
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        self.stdin.write_all(&body).unwrap();
        self.stdin.flush().unwrap();
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let response = self.wait(|m| m.get("id") == Some(&json!(id)) && m.get("method").is_none());
        assert!(
            response.get("error").is_none(),
            "{method} failed: {response}"
        );
        response["result"].clone()
    }

    /// The next message the server sends that `matches`; requests the server makes in the
    /// meantime are answered and kept.
    fn wait(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let message = match self.messages.recv_timeout(left) {
                Ok(message) => message,
                Err(RecvTimeoutError::Timeout) => panic!("no matching message in {WAIT:?}"),
                Err(RecvTimeoutError::Disconnected) => panic!("the server closed its output"),
            };
            if message.get("method").is_some() && message.get("id").is_some() {
                let id = message["id"].clone();
                self.server_requests.push(message);
                self.write(&json!({ "jsonrpc": "2.0", "id": id, "result": null }));
                continue;
            }
            if matches(&message) {
                return message;
            }
        }
    }

    fn initialize(&mut self, root: Option<&Path>, dynamic_watchers: bool) -> Value {
        let folders = root.map(|root| json!([{ "uri": uri(root), "name": "w" }]));
        let result = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": null,
                "workspaceFolders": folders,
                "capabilities": { "workspace": { "didChangeWatchedFiles": {
                    "dynamicRegistration": dynamic_watchers } } },
            }),
        );
        self.notify("initialized", json!({}));
        result
    }

    /// Initializes a client that says whether it reads nested outlines.
    fn initialize_outline(&mut self, root: &Path, hierarchical: bool) -> Value {
        let result = self.request(
            "initialize",
            json!({
                "processId": null,
                "rootUri": null,
                "workspaceFolders": [{ "uri": uri(root), "name": "w" }],
                "capabilities": { "textDocument": { "documentSymbol": {
                    "hierarchicalDocumentSymbolSupport": hierarchical } } },
            }),
        );
        self.notify("initialized", json!({}));
        result
    }

    fn outline(&mut self, path: &Path) -> Value {
        self.request(
            "textDocument/documentSymbol",
            json!({ "textDocument": { "uri": uri(path) } }),
        )
    }

    /// The `error` of the response to a request, for requests that are expected to fail.
    fn request_error(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let response = self.wait(|m| m.get("id") == Some(&json!(id)) && m.get("method").is_none());
        response["error"].clone()
    }

    fn at(path: &Path, line: u32, character: u32) -> Value {
        json!({ "textDocument": { "uri": uri(path) },
                "position": { "line": line, "character": character } })
    }

    fn hover(&mut self, path: &Path, line: u32, character: u32) -> Value {
        self.request(
            "textDocument/hover",
            json!({ "textDocument": { "uri": uri(path) },
                    "position": { "line": line, "character": character } }),
        )
    }

    fn definition(&mut self, path: &Path, line: u32, character: u32) -> Value {
        self.request(
            "textDocument/definition",
            json!({ "textDocument": { "uri": uri(path) },
                    "position": { "line": line, "character": character } }),
        )
    }

    fn references(&mut self, path: &Path, line: u32, character: u32, declaration: bool) -> Value {
        self.request(
            "textDocument/references",
            json!({ "textDocument": { "uri": uri(path) },
                    "position": { "line": line, "character": character },
                    "context": { "includeDeclaration": declaration } }),
        )
    }

    fn open(&mut self, path: &Path, version: i32, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": {
                "uri": uri(path), "languageId": "typescript", "version": version, "text": text } }),
        );
    }

    fn change_full(&mut self, path: &Path, version: i32, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({ "textDocument": { "uri": uri(path), "version": version },
                    "contentChanges": [{ "text": text }] }),
        );
    }

    /// The next diagnostics message for `path`.
    fn diagnostics(&mut self, path: &Path) -> Value {
        let want = uri(path);
        self.wait(move |m| {
            m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == want
        })["params"]
            .clone()
    }

    fn shutdown(mut self) -> std::process::ExitStatus {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the server did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn uri(path: &Path) -> String {
    tower_lsp_server::ls_types::Uri::from_file_path(path)
        .unwrap()
        .to_string()
}

fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::write(
        root.join("tsconfig.json"),
        r#"{ "compilerOptions": { "strict": true }, "include": ["*.ts"] }"#,
    )
    .unwrap();
    for (name, text) in files {
        std::fs::write(root.join(name), text).unwrap();
    }
    (dir, root)
}

#[test]
fn initialize_advertises_incremental_sync_and_names_the_server() {
    let (_dir, root) = project(&[]);
    let mut client = Client::start();
    let result = client.initialize(Some(&root), false);
    let sync = &result["capabilities"]["textDocumentSync"];
    assert_eq!(sync["openClose"], true);
    assert_eq!(sync["change"], 2, "incremental");
    assert_eq!(result["serverInfo"]["name"], "tsox-lsp");
    assert!(client.shutdown().success());
}

#[test]
fn an_error_in_an_opened_file_is_published_with_its_version() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 7, BAD);

    let params = client.diagnostics(&a);
    assert_eq!(params["version"], 7);
    let list = params["diagnostics"].as_array().unwrap();
    assert_eq!(list.len(), 1, "{params}");
    assert_eq!(list[0]["source"], "tsox");
    assert_eq!(list[0]["severity"], 1);
    assert_eq!(list[0]["code"], "TS2322");
    assert_eq!(
        list[0]["range"]["start"],
        json!({ "line": 0, "character": 25 })
    );
    assert!(client.shutdown().success());
}

#[test]
fn an_incremental_edit_that_fixes_the_error_clears_it() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, BAD);
    assert_eq!(
        client.diagnostics(&a)["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Replace `'x'` (columns 25 to 28) with `1`.
    client.notify(
        "textDocument/didChange",
        json!({ "textDocument": { "uri": uri(&a), "version": 2 },
                "contentChanges": [{
                    "range": { "start": { "line": 0, "character": 25 },
                               "end": { "line": 0, "character": 28 } },
                    "text": "1" }] }),
    );
    let params = client.diagnostics(&a);
    assert_eq!(params["version"], 2);
    assert_eq!(params["diagnostics"], json!([]));
    assert!(client.shutdown().success());
}

#[test]
fn closing_a_document_clears_its_diagnostics() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, BAD);
    assert_eq!(
        client.diagnostics(&a)["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    client.notify(
        "textDocument/didClose",
        json!({ "textDocument": { "uri": uri(&a) } }),
    );
    let params = client.diagnostics(&a);
    assert_eq!(params["diagnostics"], json!([]));
    assert!(client.shutdown().success());
}

#[test]
fn a_burst_of_changes_is_applied_in_order_and_settles_on_the_last() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, GOOD);

    for version in 2..=60 {
        client.change_full(&a, version, if version % 2 == 0 { BAD } else { GOOD });
    }
    client.change_full(&a, 61, BAD);

    // Whatever was published on the way, the last word is version 61 with its error, and a
    // version is never published after a newer one.
    let mut last = 0;
    loop {
        let params = client.diagnostics(&a);
        let version = params["version"].as_i64().unwrap();
        assert!(version >= last, "version {version} after {last}");
        last = version;
        if version == 61 {
            assert_eq!(params["diagnostics"].as_array().unwrap().len(), 1);
            break;
        }
    }
    assert!(client.shutdown().success());
}

#[test]
fn a_change_in_one_file_is_reported_in_the_file_that_imports_it() {
    let (_dir, root) = project(&[
        ("a.ts", "export const value: number = 1;\n"),
        (
            "b.ts",
            "import { value } from './a';\nexport const s: number = value;\n",
        ),
    ]);
    let (a, b) = (root.join("a.ts"), root.join("b.ts"));
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, "export const value: number = 1;\n");
    client.open(
        &b,
        1,
        "import { value } from './a';\nexport const s: number = value;\n",
    );
    client.change_full(&a, 2, "export const value: string = 'x';\n");

    let params = client.diagnostics(&b);
    assert_eq!(
        params["diagnostics"].as_array().unwrap().len(),
        1,
        "{params}"
    );
    assert_eq!(params["diagnostics"][0]["range"]["start"]["line"], 1);
    assert!(client.shutdown().success());
}

#[test]
fn a_client_that_can_watch_files_is_asked_to() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), true);
    client.open(&a, 1, BAD);
    let _ = client.diagnostics(&a);

    let registration = client
        .server_requests
        .iter()
        .find(|r| r["method"] == "client/registerCapability")
        .expect("a registration request")
        .clone();
    let entry = &registration["params"]["registrations"][0];
    assert_eq!(entry["method"], "workspace/didChangeWatchedFiles");
    assert!(
        entry["registerOptions"]["watchers"]
            .as_array()
            .unwrap()
            .len()
            >= 3
    );
    assert!(client.shutdown().success());
}

#[test]
fn a_file_added_on_disk_is_picked_up_when_the_client_reports_it() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let (a, c) = (root.join("a.ts"), root.join("c.ts"));
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, GOOD);
    // A first report of the project: a clean file reports nothing, so ask for it to be seen
    // by opening a file that is not clean.
    std::fs::write(&c, GOOD).unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": uri(&c), "type": 1 }] }),
    );
    client.open(&c, 1, BAD);
    let params = client.diagnostics(&c);
    assert_eq!(
        params["diagnostics"].as_array().unwrap().len(),
        1,
        "{params}"
    );
    assert!(client.shutdown().success());
}

#[test]
fn documents_that_are_not_files_and_unknown_changes_do_not_hurt() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);

    client.notify(
        "textDocument/didOpen",
        json!({ "textDocument": { "uri": "untitled:Untitled-1", "languageId": "typescript",
                                  "version": 1, "text": BAD } }),
    );
    // A change for a document that was never opened; the server says so in its log.
    client.change_full(&root.join("never-opened.ts"), 2, BAD);
    client.wait(|m| m["method"] == "window/logMessage");

    // Still serving.
    client.open(&a, 1, BAD);
    assert_eq!(
        client.diagnostics(&a)["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(client.shutdown().success());
}

#[test]
fn a_workspace_without_tsconfig_still_checks_open_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let a = root.join("a.ts");
    std::fs::write(&a, GOOD).unwrap();
    let mut client = Client::start();
    client.initialize(None, false);
    client.open(&a, 1, BAD);
    assert_eq!(
        client.diagnostics(&a)["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(client.shutdown().success());
}

#[test]
fn the_binary_answers_version_and_rejects_unknown_arguments() {
    let bin = env!("CARGO_BIN_EXE_tsox-lsp");
    let out = Command::new(bin).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("tsox-lsp "));
    let out = Command::new(bin).arg("--nope").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn a_client_that_cannot_watch_files_is_not_asked_to_and_exit_never_waits() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, BAD);
    let _ = client.diagnostics(&a);
    assert!(
        client.server_requests.is_empty(),
        "unexpected requests: {:?}",
        client.server_requests
    );
    assert!(client.shutdown().success());
}

#[test]
fn exit_does_not_wait_for_a_client_that_never_answers_a_registration() {
    let (_dir, root) = project(&[]);
    let mut client = Client::start();
    client.initialize(Some(&root), true);
    // The registration request is never read or answered: shutdown still completes.
    client.write(&json!({ "jsonrpc": "2.0", "id": 900, "method": "shutdown" }));
    client.notify("exit", Value::Null);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = client.child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "the server did not exit");
        std::thread::sleep(Duration::from_millis(20));
    }
}

const OUTLINE_SOURCE: &str =
    "export class Box {\n  size = 1;\n  grow() {}\n}\nexport const make = () => new Box();\n";

#[test]
fn the_server_offers_document_symbols() {
    let (_dir, root) = project(&[]);
    let mut client = Client::start();
    let result = client.initialize(Some(&root), false);
    assert_eq!(result["capabilities"]["documentSymbolProvider"], true);
    assert!(client.shutdown().success());
}

#[test]
fn a_nested_outline_has_members_under_their_class() {
    let (_dir, root) = project(&[("a.ts", OUTLINE_SOURCE)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize_outline(&root, true);
    client.open(&a, 1, OUTLINE_SOURCE);

    let outline = client.outline(&a);
    let top = outline.as_array().unwrap();
    assert_eq!(top.len(), 2, "{outline}");
    assert_eq!(top[0]["name"], "Box");
    assert_eq!(top[0]["kind"], 5, "class");
    assert_eq!(
        top[0]["selectionRange"]["start"],
        json!({ "line": 0, "character": 13 })
    );
    assert_eq!(top[0]["range"]["end"]["line"], 3);
    let members: Vec<_> = top[0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| (m["name"].as_str().unwrap(), m["kind"].as_i64().unwrap()))
        .collect();
    assert_eq!(members, vec![("size", 7), ("grow", 6)]);
    assert_eq!(top[1]["name"], "make");
    assert_eq!(top[1]["kind"], 12, "an arrow function is a function");
    assert!(client.shutdown().success());
}

#[test]
fn a_client_without_nested_outlines_gets_the_flat_form() {
    let (_dir, root) = project(&[("a.ts", OUTLINE_SOURCE)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize_outline(&root, false);
    client.open(&a, 1, OUTLINE_SOURCE);

    let outline = client.outline(&a);
    let flat = outline.as_array().unwrap();
    let names: Vec<_> = flat.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["Box", "size", "grow", "make"]);
    assert_eq!(flat[1]["containerName"], "Box");
    assert_eq!(flat[1]["location"]["uri"], uri(&a));
    assert!(client.shutdown().success());
}

#[test]
fn the_outline_follows_unsaved_edits() {
    let (_dir, root) = project(&[("a.ts", "export const one = 1;\n")]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize_outline(&root, true);
    client.open(&a, 1, "export const one = 1;\n");
    assert_eq!(client.outline(&a).as_array().unwrap().len(), 1);

    client.change_full(&a, 2, "export const one = 1;\nexport function two() {}\n");
    let outline = client.outline(&a);
    let names: Vec<_> = outline
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["one", "two"]);
    assert!(client.shutdown().success());
}

#[test]
fn a_half_typed_declaration_keeps_the_outline_of_what_came_before() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize_outline(&root, true);
    client.open(&a, 1, "export function done() {}\nclass Half {\n  m() {\n");

    let outline = client.outline(&a);
    let names: Vec<_> = outline
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["done"]);
    assert!(client.shutdown().success());
}

#[test]
fn an_outline_of_a_document_that_is_not_open_is_null() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let mut client = Client::start();
    client.initialize_outline(&root, true);
    assert_eq!(client.outline(&root.join("a.ts")), Value::Null);
    assert!(client.shutdown().success());
}

#[test]
fn the_dialect_comes_from_the_file_extension() {
    let (_dir, root) = project(&[]);
    let tsx = root.join("view.tsx");
    std::fs::write(&tsx, "").unwrap();
    let mut client = Client::start();
    client.initialize_outline(&root, true);
    client.open(
        &tsx,
        1,
        "export const view = <div />;\nexport function App() { return view; }\n",
    );
    let outline = client.outline(&tsx);
    let names: Vec<_> = outline
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["view", "App"]);
    assert!(client.shutdown().success());
}

const LIB: &str = "export function greet(name: string) { return name; }\n";
const MAIN: &str = "import { greet } from './lib';\ngreet('x');\n";

fn range_of(location: &Value) -> ((i64, i64), (i64, i64)) {
    let r = &location["range"];
    (
        (
            r["start"]["line"].as_i64().unwrap(),
            r["start"]["character"].as_i64().unwrap(),
        ),
        (
            r["end"]["line"].as_i64().unwrap(),
            r["end"]["character"].as_i64().unwrap(),
        ),
    )
}

#[test]
fn the_server_offers_definition_and_references() {
    let (_dir, root) = project(&[]);
    let mut client = Client::start();
    let result = client.initialize(Some(&root), false);
    assert_eq!(result["capabilities"]["definitionProvider"], true);
    assert_eq!(result["capabilities"]["referencesProvider"], true);
    assert!(client.shutdown().success());
}

#[test]
fn a_call_leads_to_the_function_in_the_file_it_was_imported_from() {
    let (_dir, root) = project(&[("lib.ts", LIB), ("main.ts", MAIN)]);
    let (lib, main) = (root.join("lib.ts"), root.join("main.ts"));
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    // Only main.ts is open: lib.ts is read from disk.
    client.open(&main, 1, MAIN);

    let found = client.definition(&main, 1, 2);
    let places = found.as_array().unwrap();
    assert_eq!(places.len(), 1, "{found}");
    assert_eq!(places[0]["uri"], uri(&lib));
    assert_eq!(range_of(&places[0]), ((0, 16), (0, 21)));
    // The same from the name inside the braces of the import.
    assert_eq!(client.definition(&main, 0, 10), found);
    assert!(client.shutdown().success());
}

#[test]
fn a_definition_reads_the_unsaved_text_of_the_file_it_leads_to() {
    let (_dir, root) = project(&[("lib.ts", LIB), ("main.ts", MAIN)]);
    let (lib, main) = (root.join("lib.ts"), root.join("main.ts"));
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&main, 1, MAIN);
    // lib.ts is open and edited: the function is now on the third line.
    client.open(&lib, 1, &format!("// one\n// two\n{LIB}"));

    let found = client.definition(&main, 1, 2);
    assert_eq!(range_of(&found[0]), ((2, 16), (2, 21)));
    assert!(client.shutdown().success());
}

#[test]
fn a_local_name_leads_to_its_declaration_and_columns_are_utf16() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    // The emoji is two UTF-16 units (four bytes): `value` is at column 22 on line 0 and at
    // column 16 on line 1.
    let text = "const s = '\u{1F600}'; const value = 1;\nconst t = '\u{1F600}'; value;\n";
    client.open(&a, 1, text);
    let found = client.definition(&a, 1, 18);
    assert_eq!(found.as_array().unwrap().len(), 1, "{found}");
    assert_eq!(range_of(&found[0]), ((0, 22), (0, 27)));
    assert!(client.shutdown().success());
}

#[test]
fn nothing_under_the_cursor_and_documents_not_open_are_null() {
    let (_dir, root) = project(&[("a.ts", "const a = 1;\n")]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    assert_eq!(client.definition(&a, 0, 6), Value::Null, "not open");
    assert_eq!(client.references(&a, 0, 6, true), Value::Null, "not open");
    client.open(&a, 1, "const a = 1;\nconsole.log(a);\n");
    assert_eq!(
        client.definition(&a, 1, 3),
        Value::Null,
        "a global has no definition"
    );
    assert_eq!(client.definition(&a, 0, 11), Value::Null, "a literal");
    assert!(client.shutdown().success());
}

#[test]
fn references_include_the_files_that_import_the_name() {
    let (_dir, root) = project(&[
        ("lib.ts", LIB),
        ("main.ts", MAIN),
        (
            "alias.ts",
            "import { greet as hi } from './lib';\nhi('a'); hi('b');\n",
        ),
        ("unrelated.ts", "export function greet() {}\ngreet();\n"),
    ]);
    let (lib, main, alias, unrelated) = (
        root.join("lib.ts"),
        root.join("main.ts"),
        root.join("alias.ts"),
        root.join("unrelated.ts"),
    );
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&lib, 1, LIB);
    // The project opens with the first document; wait for it before asking about the rest.
    client.open(&unrelated, 1, "export function greet() {}\ngreet();\n");

    let found = client.references(&lib, 0, 18, true);
    let places = found.as_array().unwrap();
    let in_file = |file: &Path| places.iter().filter(|p| p["uri"] == uri(file)).count();
    assert_eq!(in_file(&lib), 1, "the declaration: {found}");
    assert_eq!(in_file(&main), 2, "import list and call: {found}");
    assert_eq!(in_file(&alias), 3, "imported name and two calls: {found}");
    assert_eq!(in_file(&unrelated), 0, "a different greet: {found}");

    let without = client.references(&lib, 0, 18, false);
    let places = without.as_array().unwrap();
    assert!(
        !places.iter().any(|p| p["uri"] == uri(&lib)),
        "no declaration: {without}"
    );
    assert!(client.shutdown().success());
}

#[test]
fn references_asked_from_a_use_find_the_declaration_and_the_other_uses() {
    let (_dir, root) = project(&[("lib.ts", LIB), ("main.ts", MAIN)]);
    let (lib, main) = (root.join("lib.ts"), root.join("main.ts"));
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&main, 1, MAIN);

    let found = client.references(&main, 1, 2, true);
    let places = found.as_array().unwrap();
    assert!(places.iter().any(|p| p["uri"] == uri(&lib)), "{found}");
    assert_eq!(
        places.iter().filter(|p| p["uri"] == uri(&main)).count(),
        2,
        "{found}"
    );
    assert!(client.shutdown().success());
}

#[test]
fn a_tsconfig_paths_alias_is_resolved_like_the_project_resolves_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("tsconfig.json"),
        r#"{ "compilerOptions": { "strict": true, "baseUrl": ".", "paths": { "@lib/*": ["src/*"] } },
             "include": ["*.ts", "src/*.ts"] }"#,
    )
    .unwrap();
    let util = root.join("src/util.ts");
    std::fs::write(&util, "export const answer = 42;\n").unwrap();
    let main = root.join("main.ts");
    let text = "import { answer } from '@lib/util';\nanswer;\n";
    std::fs::write(&main, text).unwrap();

    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&main, 1, text);
    let found = client.definition(&main, 1, 2);
    assert_eq!(found[0]["uri"], uri(&util), "{found}");
    assert_eq!(range_of(&found[0]), ((0, 13), (0, 19)));
    assert!(client.shutdown().success());
}

#[test]
fn a_file_that_appears_later_is_found_after_the_client_reports_it() {
    let (_dir, root) = project(&[]);
    let main = root.join("main.ts");
    let text = "import { later } from './later';\nlater;\n";
    std::fs::write(&main, text).unwrap();
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&main, 1, text);

    // Not there yet: the definition stays on the import.
    let before = client.definition(&main, 1, 1);
    assert_eq!(before[0]["uri"], uri(&main), "{before}");

    let later = root.join("later.ts");
    std::fs::write(&later, "export const later = 1;\n").unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({ "changes": [{ "uri": uri(&later), "type": 1 }] }),
    );
    let after = client.definition(&main, 1, 1);
    assert_eq!(after[0]["uri"], uri(&later), "{after}");
    assert!(client.shutdown().success());
}

#[test]
fn requests_are_answered_while_the_diagnostics_keep_coming() {
    let (_dir, root) = project(&[("lib.ts", LIB), ("main.ts", MAIN)]);
    let main = root.join("main.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&main, 1, MAIN);
    for version in 2..12 {
        client.change_full(&main, version, &format!("{MAIN}// {version}\n"));
        let found = client.definition(&main, 1, 2);
        assert_eq!(
            found.as_array().unwrap().len(),
            1,
            "version {version}: {found}"
        );
    }
    assert!(client.shutdown().success());
}

/// The zero-based line and UTF-16 column of the first `needle` in `text`, plus `into` columns.
fn position_of(text: &str, needle: &str, into: usize) -> (u32, u32) {
    let at = text.find(needle).expect("needle present");
    let line = text[..at].matches('\n').count() as u32;
    let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let column = text[line_start..at].encode_utf16().count() + into;
    (line, column as u32)
}

fn hover_code(hover: &Value) -> String {
    let value = hover["contents"]["value"]
        .as_str()
        .expect("markdown contents");
    value
        .strip_prefix("```ts\n")
        .and_then(|rest| rest.split("\n```").next())
        .unwrap_or(value)
        .to_owned()
}

#[test]
fn the_server_offers_hover() {
    let (_dir, root) = project(&[]);
    let mut client = Client::start();
    let result = client.initialize(Some(&root), false);
    assert_eq!(result["capabilities"]["hoverProvider"], true);
    assert!(client.shutdown().success());
}

#[test]
fn hovering_an_inferred_const_shows_the_type_the_checker_gave_it() {
    let text =
        "const total = [1, 2, 3].length + 4;\nlet label = 'x';\nconsole.log(total, label);\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);

    let (line, column) = position_of(text, "total", 2);
    let found = client.hover(&a, line, column);
    assert_eq!(hover_code(&found), "const total: number");
    assert_eq!(found["contents"]["kind"], "markdown");
    assert_eq!(
        found["range"],
        json!({ "start": { "line": 0, "character": 6 }, "end": { "line": 0, "character": 11 } })
    );
    // At a use, too, and a `let` widens its literal.
    let (line, column) = position_of(text, "label)", 0);
    assert_eq!(
        hover_code(&client.hover(&a, line, column)),
        "let label: string"
    );
    assert!(client.shutdown().success());
}

#[test]
fn hover_on_an_imported_function_shows_its_signature_and_documentation() {
    let lib = "/**\n * Greets someone.\n * @param who - the person\n */\nexport function greet(who: string): string { return 'hi ' + who; }\n";
    let main = "import { greet } from './lib';\nconst message = greet('x');\n";
    let (_dir, root) = project(&[("lib.ts", lib), ("main.ts", main)]);
    let main_path = root.join("main.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&main_path, 1, main);

    let (line, column) = position_of(main, "greet('x')", 1);
    let found = client.hover(&main_path, line, column);
    let value = found["contents"]["value"].as_str().unwrap();
    assert!(
        value.starts_with("```ts\nfunction greet(who: string): string\n```"),
        "{value}"
    );
    assert!(value.contains("Greets someone."), "{value}");
    assert!(value.contains("*@param* `who` — the person"), "{value}");

    // The result of the call is typed by the checker through the import.
    let (line, column) = position_of(main, "message", 0);
    assert_eq!(
        hover_code(&client.hover(&main_path, line, column)),
        "const message: string"
    );
    assert!(client.shutdown().success());
}

#[test]
fn hover_shows_a_function_return_type_it_had_to_infer() {
    let text = "function add(a: number, b: number) { return a + b; }\nadd(1, 2);\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);
    let (line, column) = position_of(text, "add(1", 1);
    assert_eq!(
        hover_code(&client.hover(&a, line, column)),
        "function add(a: number, b: number): number"
    );
    assert!(client.shutdown().success());
}

#[test]
fn hover_on_a_property_access_shows_the_type_of_the_access() {
    let text = "const o = { count: 1, name: 'n' };\nconst c = o.count;\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);
    let (line, column) = position_of(text, "o.count", 3);
    assert_eq!(
        hover_code(&client.hover(&a, line, column)),
        "(property) count: number"
    );
    assert!(client.shutdown().success());
}

#[test]
fn hover_after_an_edit_is_about_the_edited_text() {
    let before = "const v = 1;\nlet w = v;\n";
    let (_dir, root) = project(&[("a.ts", before)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, before);
    let (line, column) = position_of(before, "w =", 0);
    assert_eq!(hover_code(&client.hover(&a, line, column)), "let w: number");

    // Asked straight after an edit, without waiting for diagnostics: the answer waits for
    // the check of the edit.
    let after = "const v = 'text';\nlet w = v;\n";
    client.change_full(&a, 2, after);
    let (line, column) = position_of(after, "w =", 0);
    assert_eq!(hover_code(&client.hover(&a, line, column)), "let w: string");
    assert!(client.shutdown().success());
}

#[test]
fn hover_columns_are_utf16() {
    let text = "const s = '\u{1F600}'; const after = 5 + 5;\nafter;\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);
    let (line, column) = position_of(text, "after =", 1);
    let found = client.hover(&a, line, column);
    assert_eq!(hover_code(&found), "const after: number");
    // `after` starts at column 22 in UTF-16 units; it is at byte 24.
    assert_eq!(found["range"]["start"]["character"], 22);
    assert_eq!(found["range"]["end"]["character"], 27);
    assert!(client.shutdown().success());
}

#[test]
fn nothing_to_show_is_null() {
    let text = "const n = 1 + 2; // note\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    assert_eq!(client.hover(&a, 0, 6), Value::Null, "not open");
    client.open(&a, 1, text);
    for needle in ["+ 2", "const", "note"] {
        let (line, column) = position_of(text, needle, 0);
        assert_eq!(client.hover(&a, line, column), Value::Null, "on `{needle}`");
    }
    assert!(client.shutdown().success());
}

#[test]
fn hover_is_answered_in_the_middle_of_a_burst_of_edits() {
    let (_dir, root) = project(&[("a.ts", GOOD)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, "let v = 1;\n");
    for version in 2..40 {
        let text = if version % 2 == 0 {
            "let v = 'a';\n"
        } else {
            "let v = 1;\n"
        };
        client.change_full(&a, version, text);
    }
    client.change_full(&a, 40, "let v = true;\n");
    let found = client.hover(&a, 0, 4);
    assert_eq!(hover_code(&found), "let v: boolean");
    assert!(client.shutdown().success());
}

#[test]
fn hover_on_a_method_being_called_shows_its_signature() {
    let text = "class Box { get(n: number): string { return String(n); } }\nconst box = new Box();\nconst got = box.get(1);\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);
    let (line, column) = position_of(text, "box.get(1)", 5);
    assert_eq!(
        hover_code(&client.hover(&a, line, column)),
        "(method) get(n: number): string"
    );
    let (line, column) = position_of(text, "get(n", 1);
    assert_eq!(
        hover_code(&client.hover(&a, line, column)),
        "(method) get(n: number): string",
        "and where it is declared"
    );
    assert!(client.shutdown().success());
}

fn edits_by_file(rename: &Value) -> std::collections::BTreeMap<String, Vec<(String, String)>> {
    let mut out = std::collections::BTreeMap::new();
    for (file, edits) in rename["changes"].as_object().expect("changes") {
        let mut list: Vec<(String, String)> = edits
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    format!(
                        "{}:{}-{}:{}",
                        e["range"]["start"]["line"],
                        e["range"]["start"]["character"],
                        e["range"]["end"]["line"],
                        e["range"]["end"]["character"]
                    ),
                    e["newText"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        list.sort();
        let name = file.rsplit('/').next().unwrap().to_owned();
        out.insert(name, list);
    }
    out
}

#[test]
fn the_server_offers_the_structure_and_editing_features() {
    let (_dir, root) = project(&[]);
    let mut client = Client::start();
    let caps = client.initialize(Some(&root), false)["capabilities"].clone();
    assert_eq!(caps["documentHighlightProvider"], true);
    assert_eq!(caps["foldingRangeProvider"], true);
    assert_eq!(caps["selectionRangeProvider"], true);
    assert_eq!(caps["workspaceSymbolProvider"], true);
    assert_eq!(caps["renameProvider"]["prepareProvider"], true);
    assert!(client.shutdown().success());
}

#[test]
fn highlights_mark_the_writes_and_the_reads() {
    let text = "let v = 1;\nv = 2;\nconsole.log(v);\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);
    let found = client.request("textDocument/documentHighlight", Client::at(&a, 2, 12));
    let kinds: Vec<(i64, i64)> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["range"]["start"]["line"].as_i64().unwrap(),
                h["kind"].as_i64().unwrap(),
            )
        })
        .collect();
    // LSP kinds: 2 is read, 3 is write.
    assert_eq!(kinds, vec![(0, 3), (1, 3), (2, 2)]);
    assert_eq!(
        client.request("textDocument/documentHighlight", Client::at(&a, 2, 3)),
        Value::Null,
        "a global"
    );
    assert!(client.shutdown().success());
}

#[test]
fn folding_ranges_cover_bodies_imports_and_comments() {
    let text = "import a from './a';\nimport b from './b';\n/**\n * doc\n */\nfunction f() {\n  one();\n  two();\n}\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);
    let found = client.request(
        "textDocument/foldingRange",
        json!({ "textDocument": { "uri": uri(&a) } }),
    );
    let ranges: Vec<(i64, i64, Option<String>)> = found
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["startLine"].as_i64().unwrap(),
                r["endLine"].as_i64().unwrap(),
                r["kind"].as_str().map(str::to_owned),
            )
        })
        .collect();
    assert_eq!(
        ranges,
        vec![
            (0, 1, Some("imports".to_owned())),
            (2, 4, Some("comment".to_owned())),
            (5, 7, None),
        ]
    );
    assert!(client.shutdown().success());
}

#[test]
fn a_selection_grows_through_its_enclosing_ranges() {
    let text = "function f(a: number) { return a + 1; }\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);
    let (line, column) = position_of(text, "a + 1", 0);
    let found = client.request(
        "textDocument/selectionRange",
        json!({ "textDocument": { "uri": uri(&a) },
                "positions": [{ "line": line, "character": column }] }),
    );
    // Each range as ((start line, start column), (end line, end column)), innermost first.
    let mut ranges = Vec::new();
    let mut node = &found[0];
    loop {
        let r = &node["range"];
        ranges.push((
            (
                r["start"]["line"].as_i64().unwrap(),
                r["start"]["character"].as_i64().unwrap(),
            ),
            (
                r["end"]["line"].as_i64().unwrap(),
                r["end"]["character"].as_i64().unwrap(),
            ),
        ));
        if node["parent"].is_null() {
            break;
        }
        node = &node["parent"];
    }
    let column = column as i64;
    assert_eq!(ranges[0], ((0, column), (0, column + 1)), "the name");
    assert_eq!(*ranges.last().unwrap(), ((0, 0), (1, 0)), "the whole file");
    assert!(
        ranges
            .windows(2)
            .all(|w| w[1].0 <= w[0].0 && w[1].1 >= w[0].1)
    );
    assert!(client.shutdown().success());
}

#[test]
fn workspace_symbols_search_files_that_are_not_open() {
    let (_dir, root) = project(&[
        (
            "lib.ts",
            "export class Greeter { greet() {} }\nexport const other = 1;\n",
        ),
        ("main.ts", "export function greetAll() {}\n"),
    ]);
    let (lib, main) = (root.join("lib.ts"), root.join("main.ts"));
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    // Opening one file opens the project; the other is found through it.
    client.open(&main, 1, "export function greetAll() {}\n");
    let found = client.request("workspace/symbol", json!({ "query": "greet" }));
    let list = found.as_array().unwrap();
    let names: Vec<&str> = list.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["greet", "Greeter", "greetAll"]);
    assert_eq!(list[0]["containerName"], "Greeter");
    assert_eq!(list[0]["location"]["uri"], uri(&lib));
    assert_eq!(list[2]["location"]["uri"], uri(&main));
    assert!(!names.contains(&"other"));
    assert!(client.shutdown().success());
}

#[test]
fn prepare_rename_gives_the_name_or_nothing() {
    let text = "const total = 1;\nconsole.log(total);\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);
    let found = client.request("textDocument/prepareRename", Client::at(&a, 1, 14));
    assert_eq!(found["placeholder"], "total");
    assert_eq!(
        found["range"],
        json!({ "start": { "line": 1, "character": 12 }, "end": { "line": 1, "character": 17 } })
    );
    assert_eq!(
        client.request("textDocument/prepareRename", Client::at(&a, 1, 3)),
        Value::Null,
        "a global"
    );
    assert!(client.shutdown().success());
}

#[test]
fn renaming_an_export_edits_the_declaration_and_the_imports_only() {
    let lib = "export function greet() {}\ngreet();\n";
    let main = "import { greet } from './lib';\ngreet();\n";
    let alias = "import { greet as hi } from './lib';\nhi();\n";
    let (_dir, root) = project(&[("lib.ts", lib), ("main.ts", main), ("alias.ts", alias)]);
    let main_path = root.join("main.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    // Only main.ts is open: the others are edited on disk's text.
    client.open(&main_path, 1, main);

    let mut params = Client::at(&main_path, 1, 2);
    params["newName"] = json!("hello");
    let found = client.request("textDocument/rename", params);
    let edits = edits_by_file(&found);
    assert_eq!(
        edits["lib.ts"],
        vec![
            ("0:16-0:21".to_owned(), "hello".to_owned()),
            ("1:0-1:5".to_owned(), "hello".to_owned()),
        ]
    );
    assert_eq!(
        edits["main.ts"],
        vec![("0:9-0:14".to_owned(), "hello as greet".to_owned())],
        "the import is aliased, the use is left alone"
    );
    assert_eq!(
        edits["alias.ts"],
        vec![("0:9-0:14".to_owned(), "hello".to_owned())],
        "an aliased import changes only the imported name"
    );
    assert!(client.shutdown().success());
}

#[test]
fn a_rename_the_server_refuses_says_why() {
    let lib_dir = tempfile::tempdir().unwrap();
    let _ = lib_dir;
    let text = "const a = 1;\nconst b = 2;\na;\nconsole.log(b);\n";
    let (_dir, root) = project(&[("a.ts", text)]);
    let a = root.join("a.ts");
    let mut client = Client::start();
    client.initialize(Some(&root), false);
    client.open(&a, 1, text);

    let mut bad = Client::at(&a, 2, 0);
    bad["newName"] = json!("not valid");
    let error = client.request_error("textDocument/rename", bad);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("not a valid identifier"),
        "{error}"
    );

    let mut clash = Client::at(&a, 2, 0);
    clash["newName"] = json!("b");
    let error = client.request_error("textDocument/rename", clash);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("already declared"),
        "{error}"
    );

    let mut global = Client::at(&a, 3, 2);
    global["newName"] = json!("log2");
    assert_eq!(
        client.request("textDocument/rename", global),
        Value::Null,
        "a global"
    );
    assert!(client.shutdown().success());
}
