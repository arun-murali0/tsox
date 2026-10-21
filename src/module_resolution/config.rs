use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

// A project's `tsconfig.json` as the engine reads it: the file and everything it extends,
// merged the way TypeScript merges them, and the list of files it selects.
//
// Problem: the engine took every `.ts` file under a directory and read the config only as
// text, so `include`, `exclude` and `files` selected nothing, and an `extends` chain could
// not be seen at all.
// Picked: read the config here, with no new dependency. A config is JSON with comments and
// trailing commas, so there is a small reader for that. Inheritance follows TypeScript:
// `compilerOptions` merge key by key with the extending file winning, and `files`,
// `include` and `exclude` are replaced as a whole by the extending file. A relative
// pattern is relative to the config file that wrote it. Selection follows TypeScript's
// rules: a pattern with no extension and no wildcard in its last segment names a
// directory, `*`, `?` and `**` never enter a dot-directory or `node_modules`,
// `bower_components` or `jspm_packages`, and when a name exists as `.ts`, `.tsx` and
// `.d.ts`, only the highest of them is kept.
// Now: `ProjectConfig::load` gives the effective options and `entry_files` the files the
// project selects, and a config that cannot be read is an error, not silently empty.

/// Why a configuration could not be read.
#[derive(Debug)]
pub enum ConfigError {
    Read { path: PathBuf, reason: String },
    Parse { path: PathBuf, reason: String },
    ExtendsNotFound { from: PathBuf, specifier: String },
    ExtendsCycle { path: PathBuf },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, reason } => write!(f, "cannot read {}: {reason}", path.display()),
            Self::Parse { path, reason } => {
                write!(f, "cannot parse {}: {reason}", path.display())
            }
            Self::ExtendsNotFound { from, specifier } => write!(
                f,
                "{} extends '{specifier}', which was not found",
                from.display()
            ),
            Self::ExtendsCycle { path } => {
                write!(f, "{} is extended by itself", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// A parsed JSON value. Objects keep the order they were written in.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(text) => Some(text),
            _ => None,
        }
    }

    fn as_strings(&self) -> Option<Vec<String>> {
        match self {
            Json::Array(items) => items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect(),
            _ => None,
        }
    }
}

/// Parses JSON that may contain `//` and `/* */` comments, a trailing comma before `]` or
/// `}`, and a leading byte-order mark, which is what `tsconfig.json` files contain.
pub fn parse_jsonc(text: &str) -> Result<Json, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut parser = JsonParser {
        bytes: text.as_bytes(),
        text,
        at: 0,
    };
    parser.skip_trivia()?;
    let value = parser.value(0)?;
    parser.skip_trivia()?;
    if parser.at != parser.bytes.len() {
        return Err(parser.error("unexpected text after the value"));
    }
    Ok(value)
}

struct JsonParser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
}

const MAX_DEPTH: usize = 64;

impl JsonParser<'_> {
    fn error(&self, message: &str) -> String {
        let line = self.bytes[..self.at.min(self.bytes.len())]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1;
        format!("{message} (line {line})")
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_trivia(&mut self) -> Result<(), String> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r' | b'\n') => self.at += 1,
                Some(b'/') => match self.bytes.get(self.at + 1) {
                    Some(b'/') => {
                        while !matches!(self.peek(), None | Some(b'\n')) {
                            self.at += 1;
                        }
                    }
                    Some(b'*') => {
                        self.at += 2;
                        loop {
                            match self.peek() {
                                None => return Err(self.error("unterminated comment")),
                                Some(b'*') if self.bytes.get(self.at + 1) == Some(&b'/') => {
                                    self.at += 2;
                                    break;
                                }
                                Some(_) => self.at += 1,
                            }
                        }
                    }
                    _ => return Err(self.error("unexpected '/'")),
                },
                _ => return Ok(()),
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return Err(self.error("nested too deeply"));
        }
        match self.peek() {
            None => Err(self.error("unexpected end of input")),
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => Ok(Json::String(self.string()?)),
            Some(b't') => self.word("true", Json::Bool(true)),
            Some(b'f') => self.word("false", Json::Bool(false)),
            Some(b'n') => self.word("null", Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.error("unexpected character")),
        }
    }

    fn word(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("unexpected word"))
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.at;
        while matches!(
            self.peek(),
            Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')
        ) {
            self.at += 1;
        }
        self.text[start..self.at]
            .parse::<f64>()
            .map(Json::Number)
            .map_err(|_| self.error("invalid number"))
    }

    fn string(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut out = String::new();
        loop {
            let rest = &self.text[self.at..];
            let Some(c) = rest.chars().next() else {
                return Err(self.error("unterminated string"));
            };
            self.at += c.len_utf8();
            match c {
                '"' => return Ok(out),
                '\\' => {
                    let Some(escape) = self.text[self.at..].chars().next() else {
                        return Err(self.error("unterminated string"));
                    };
                    self.at += escape.len_utf8();
                    match escape {
                        '"' => out.push('"'),
                        '\\' => out.push('\\'),
                        '/' => out.push('/'),
                        'b' => out.push('\u{8}'),
                        'f' => out.push('\u{c}'),
                        'n' => out.push('\n'),
                        'r' => out.push('\r'),
                        't' => out.push('\t'),
                        'u' => {
                            let code = self.hex4()?;
                            if (0xd800..0xdc00).contains(&code)
                                && self.text[self.at..].starts_with("\\u")
                            {
                                self.at += 2;
                                let low = self.hex4()?;
                                let combined =
                                    0x10000 + ((code - 0xd800) << 10) + (low.wrapping_sub(0xdc00));
                                out.push(char::from_u32(combined).unwrap_or('\u{fffd}'));
                            } else {
                                out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            }
                        }
                        _ => return Err(self.error("invalid escape")),
                    }
                }
                c if (c as u32) < 0x20 => return Err(self.error("control character in string")),
                c => out.push(c),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .text
            .get(self.at..self.at + 4)
            .ok_or_else(|| self.error("short \\u escape"))?;
        let code = u32::from_str_radix(digits, 16).map_err(|_| self.error("bad \\u escape"))?;
        self.at += 4;
        Ok(code)
    }

    fn array(&mut self, depth: usize) -> Result<Json, String> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.skip_trivia()?;
            match self.peek() {
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                None => return Err(self.error("unterminated array")),
                _ => {}
            }
            items.push(self.value(depth + 1)?);
            self.skip_trivia()?;
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {}
                _ => return Err(self.error("expected ',' or ']'")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, String> {
        self.at += 1;
        let mut entries: Vec<(String, Json)> = Vec::new();
        loop {
            self.skip_trivia()?;
            match self.peek() {
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Json::Object(entries));
                }
                Some(b'"') => {}
                None => return Err(self.error("unterminated object")),
                _ => return Err(self.error("expected a string key")),
            }
            let key = self.string()?;
            self.skip_trivia()?;
            if self.peek() != Some(b':') {
                return Err(self.error("expected ':'"));
            }
            self.at += 1;
            self.skip_trivia()?;
            let value = self.value(depth + 1)?;
            // The last of two equal keys wins, as in a JSON.parse.
            entries.retain(|(k, _)| *k != key);
            entries.push((key, value));
            self.skip_trivia()?;
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {}
                _ => return Err(self.error("expected ',' or '}'")),
            }
        }
    }
}

// A list written in one config file, with the directory its relative entries are
// relative to: the directory of that file.
#[derive(Clone, Debug)]
struct Located {
    base: PathBuf,
    items: Vec<String>,
}

/// The effective configuration of a project: a `tsconfig.json` and every file it extends.
#[derive(Clone, Debug)]
pub struct ProjectConfig {
    config_file: PathBuf,
    sources: Vec<PathBuf>,
    files: Option<Located>,
    include: Option<Located>,
    exclude: Option<Located>,
    compiler_options: BTreeMap<String, Json>,
    out_dir: Option<PathBuf>,
}

impl ProjectConfig {
    /// Reads `config_file` and what it extends. The path should be absolute.
    pub fn load(config_file: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let config_file = normalize(config_file.as_ref());
        let mut chain = Vec::new();
        let layers = read_layers(&config_file, &mut chain)?;
        let mut merged = Self {
            config_file: config_file.clone(),
            sources: Vec::new(),
            files: None,
            include: None,
            exclude: None,
            compiler_options: BTreeMap::new(),
            out_dir: None,
        };
        // Base layers come first, so a later layer wins.
        for layer in layers {
            merged.sources.push(layer.path.clone());
            let base = layer
                .path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default();
            if let Some(items) = layer.json.get("files").and_then(Json::as_strings) {
                merged.files = Some(Located {
                    base: base.clone(),
                    items,
                });
            }
            if let Some(items) = layer.json.get("include").and_then(Json::as_strings) {
                merged.include = Some(Located {
                    base: base.clone(),
                    items,
                });
            }
            if let Some(items) = layer.json.get("exclude").and_then(Json::as_strings) {
                merged.exclude = Some(Located {
                    base: base.clone(),
                    items,
                });
            }
            if let Some(Json::Object(options)) = layer.json.get("compilerOptions") {
                for (key, value) in options {
                    if key == "outDir" {
                        if let Some(dir) = value.as_str() {
                            merged.out_dir = Some(normalize(&base.join(dir)));
                        }
                    }
                    merged.compiler_options.insert(key.clone(), value.clone());
                }
            }
        }
        // The file itself first, then what it extends, nearest first.
        merged.sources.reverse();
        Ok(merged)
    }

    /// The `tsconfig.json` this was loaded from.
    pub fn config_file(&self) -> &Path {
        &self.config_file
    }

    /// The directory of the config file, which is the project root.
    pub fn root(&self) -> &Path {
        self.config_file.parent().unwrap_or(Path::new("."))
    }

    /// Every config file that was read: the file itself, then each file it extends.
    pub fn sources(&self) -> &[PathBuf] {
        &self.sources
    }

    /// A compiler option as the effective configuration sets it, after `extends`.
    pub fn compiler_option(&self, name: &str) -> Option<&Json> {
        self.compiler_options.get(name)
    }

    /// The files the project selects, sorted, as absolute paths.
    pub fn entry_files(&self) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = Vec::new();

        if let Some(files) = &self.files {
            for item in &files.items {
                let path = normalize(&files.base.join(item));
                if path.is_file() {
                    found.push(path);
                }
            }
        }

        // With neither `include` nor `files`, everything under the config directory is
        // selected. With `files` and no `include`, only the listed files are.
        let include = match (&self.include, &self.files) {
            (Some(include), _) => Some(include.clone()),
            (None, None) => Some(Located {
                base: self.root().to_path_buf(),
                items: vec!["**/*".to_owned()],
            }),
            (None, Some(_)) => None,
        };

        if let Some(include) = include {
            let excludes = self.exclude_patterns();
            let patterns: Vec<Pattern> = include
                .items
                .iter()
                .map(|item| Pattern::include(&include.base, item))
                .collect();
            let mut roots: Vec<PathBuf> = patterns.iter().map(Pattern::walk_root).collect();
            roots.sort();
            roots.dedup();
            let mut candidates = Vec::new();
            for root in roots {
                walk(&root, &excludes, &mut candidates);
            }
            for path in candidates {
                if patterns.iter().any(|p| p.matches_file(&path)) {
                    found.push(path);
                }
            }
        }

        found.sort();
        found.dedup();
        keep_highest_extension(found)
    }

    fn exclude_patterns(&self) -> Vec<Pattern> {
        match &self.exclude {
            Some(exclude) => exclude
                .items
                .iter()
                .map(|item| Pattern::exclude(&exclude.base, item))
                .collect(),
            None => {
                let root = self.root();
                let mut defaults: Vec<Pattern> =
                    ["node_modules", "bower_components", "jspm_packages"]
                        .iter()
                        .map(|name| Pattern::exclude(root, name))
                        .collect();
                if let Some(out_dir) = &self.out_dir {
                    defaults.push(Pattern::exclude(out_dir, "."));
                }
                defaults
            }
        }
    }
}

struct Layer {
    path: PathBuf,
    json: Json,
}

// The layers of a config, base-most first, the file itself last. `chain` holds the files
// being read now, so a file that extends itself is an error and not a loop.
fn read_layers(path: &Path, chain: &mut Vec<PathBuf>) -> Result<Vec<Layer>, ConfigError> {
    if chain.iter().any(|seen| seen == path) {
        return Err(ConfigError::ExtendsCycle {
            path: path.to_path_buf(),
        });
    }
    let text = fs::read_to_string(path).map_err(|error| ConfigError::Read {
        path: path.to_path_buf(),
        reason: error.to_string(),
    })?;
    let json = parse_jsonc(&text).map_err(|reason| ConfigError::Parse {
        path: path.to_path_buf(),
        reason,
    })?;
    if !matches!(json, Json::Object(_)) {
        return Err(ConfigError::Parse {
            path: path.to_path_buf(),
            reason: "the top level is not an object".to_owned(),
        });
    }

    chain.push(path.to_path_buf());
    let mut layers = Vec::new();
    let specifiers: Vec<String> = match json.get("extends") {
        Some(Json::String(one)) => vec![one.clone()],
        Some(many @ Json::Array(_)) => many.as_strings().unwrap_or_default(),
        _ => Vec::new(),
    };
    for specifier in specifiers {
        let found = find_extends(path, &specifier).ok_or_else(|| ConfigError::ExtendsNotFound {
            from: path.to_path_buf(),
            specifier: specifier.clone(),
        })?;
        layers.extend(read_layers(&found, chain)?);
    }
    chain.pop();

    layers.push(Layer {
        path: path.to_path_buf(),
        json,
    });
    Ok(layers)
}

// `./base`, `../base.json` and absolute paths are files; a bare name is a package found
// in the nearest `node_modules` above the config.
fn find_extends(from: &Path, specifier: &str) -> Option<PathBuf> {
    let directory = from.parent()?;
    let spec_path = Path::new(specifier);
    if specifier.starts_with("./") || specifier.starts_with("../") || spec_path.is_absolute() {
        return json_file(&normalize(&directory.join(specifier)));
    }
    for ancestor in directory.ancestors() {
        let package = ancestor.join("node_modules").join(specifier);
        if let Some(found) = json_file(&package) {
            return Some(normalize(&found));
        }
        let inside = package.join("tsconfig.json");
        if inside.is_file() {
            return Some(normalize(&inside));
        }
    }
    None
}

fn json_file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let mut with_extension = path.as_os_str().to_owned();
    with_extension.push(".json");
    let with_extension = PathBuf::from(with_extension);
    with_extension.is_file().then_some(with_extension)
}

// Removes `.` and resolves `..` without touching the disk, so two spellings of one path
// compare equal.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

const SUPPORTED: &[&str] = &[".d.mts", ".d.cts", ".d.ts", ".mts", ".cts", ".tsx", ".ts"];

fn is_supported(name: &str) -> bool {
    SUPPORTED.iter().any(|ext| name.ends_with(ext))
}

fn is_implicitly_excluded(name: &str) -> bool {
    matches!(name, "node_modules" | "bower_components" | "jspm_packages")
}

// One include or exclude entry, split into segments under the directory it is relative to.
struct Pattern {
    base: PathBuf,
    segments: Vec<String>,
}

impl Pattern {
    fn new(base: &Path, spec: &str, is_include: bool) -> Self {
        let spec = spec.replace('\\', "/");
        let mut segments: Vec<String> = spec
            .split('/')
            .filter(|s| !s.is_empty() && *s != ".")
            .map(str::to_owned)
            .collect();
        // An absolute spec keeps the root as its base.
        let mut base = base.to_path_buf();
        if spec.starts_with('/') {
            base = PathBuf::from("/");
        }
        // `..` leaves the base directory, so it moves the base and is not matched.
        while segments.first().is_some_and(|s| s == "..") {
            segments.remove(0);
            base.pop();
        }
        if is_include {
            if let Some(last) = segments.last() {
                if last == "**" {
                    // A trailing `**` means everything under that directory.
                    segments.push("*".to_owned());
                } else if !last.contains('.') && !has_wildcard(last) {
                    // A last segment with neither a wildcard nor an extension names a
                    // directory.
                    segments.push("**".to_owned());
                    segments.push("*".to_owned());
                }
            }
        } else {
            // An exclude covers everything under a directory it matches, so a trailing
            // `**` adds nothing.
            while segments.last().is_some_and(|s| s == "**") {
                segments.pop();
            }
        }
        Self {
            base: normalize(&base),
            segments,
        }
    }

    fn include(base: &Path, spec: &str) -> Self {
        Self::new(base, spec, true)
    }

    fn exclude(base: &Path, spec: &str) -> Self {
        Self::new(base, spec, false)
    }

    // The deepest directory with no wildcard in its name: nothing outside it can match.
    fn walk_root(&self) -> PathBuf {
        let mut root = self.base.clone();
        for segment in &self.segments {
            if has_wildcard(segment) || segment.contains('.') {
                break;
            }
            root.push(segment);
        }
        root
    }

    fn relative<'a>(&self, path: &'a Path) -> Option<Vec<&'a str>> {
        let rest = path.strip_prefix(&self.base).ok()?;
        rest.components()
            .map(|c| c.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
    }

    // A file the include pattern selects.
    fn matches_file(&self, path: &Path) -> bool {
        let Some(parts) = self.relative(path) else {
            return false;
        };
        match_segments(&self.segments, &parts, true)
    }

    // A file or directory the exclude pattern covers: the entry itself or anything under
    // a directory it matches.
    fn covers(&self, path: &Path) -> bool {
        let Some(parts) = self.relative(path) else {
            return false;
        };
        if self.segments.is_empty() {
            return true;
        }
        (1..=parts.len()).any(|n| match_segments(&self.segments, &parts[..n], false))
    }
}

fn has_wildcard(segment: &str) -> bool {
    segment.contains('*') || segment.contains('?')
}

// Whether `pattern` matches all of `path`. In an include, a wildcard never matches a name
// that starts with `.` or an implicitly excluded directory; in an exclude it can.
fn match_segments(pattern: &[String], path: &[&str], is_include: bool) -> bool {
    let Some((first, rest)) = pattern.split_first() else {
        return path.is_empty();
    };
    if first == "**" {
        // Zero directories, or one directory and then `**` again. The last path part is a
        // file name, which `**` never stands for.
        if match_segments(rest, path, is_include) {
            return true;
        }
        return match path.split_first() {
            Some((dir, tail)) if !tail.is_empty() => {
                if is_include && (dir.starts_with('.') || is_implicitly_excluded(dir)) {
                    false
                } else {
                    match_segments(pattern, tail, is_include)
                }
            }
            _ => false,
        };
    }
    let Some((name, tail)) = path.split_first() else {
        return false;
    };
    if is_include && has_wildcard(first) && (name.starts_with('.') || is_implicitly_excluded(name))
    {
        return false;
    }
    glob_segment(first, name) && match_segments(rest, tail, is_include)
}

// `*` is any run of characters and `?` is one character, inside one path segment.
fn glob_segment(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    let (mut p, mut n) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) && pattern[p] != '*' {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = n;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            n = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

// Collects the supported source files under `root`, not entering a directory an exclude
// pattern covers.
fn walk(root: &Path, excludes: &[Pattern], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if excludes.iter().any(|pattern| pattern.covers(&path)) {
            continue;
        }
        let is_dir = fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            walk(&path, excludes, out);
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_supported)
        {
            out.push(path);
        }
    }
}

// When one name exists as `a.ts`, `a.tsx` and `a.d.ts`, TypeScript keeps the first of
// those and drops the rest, so the declaration file does not stand next to its source.
fn keep_highest_extension(files: Vec<PathBuf>) -> Vec<PathBuf> {
    fn family(name: &str) -> Option<(&str, usize, &'static str)> {
        for (families, tag) in [
            (&[".ts", ".tsx", ".d.ts"][..], "ts"),
            (&[".mts", ".d.mts"][..], "mts"),
            (&[".cts", ".d.cts"][..], "cts"),
        ] {
            for (rank, ext) in families.iter().enumerate() {
                if let Some(stem) = name.strip_suffix(ext) {
                    // `.d.ts` also ends in `.ts`; the longer extension is the real one.
                    if *ext == ".ts" && name.ends_with(".d.ts") {
                        continue;
                    }
                    return Some((stem, rank, tag));
                }
            }
        }
        None
    }
    let mut best: BTreeMap<(PathBuf, &'static str), (usize, PathBuf)> = BTreeMap::new();
    let mut others = Vec::new();
    for path in files {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        match family(name) {
            Some((stem, rank, tag)) => {
                let key = (path.with_file_name(stem), tag);
                match best.get(&key) {
                    Some((current, _)) if *current <= rank => {}
                    _ => {
                        best.insert(key, (rank, path));
                    }
                }
            }
            None => others.push(path),
        }
    }
    let mut kept: Vec<PathBuf> = best.into_values().map(|(_, path)| path).collect();
    kept.extend(others);
    kept.sort();
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_reads_comments_trailing_commas_and_a_byte_order_mark() {
        let text = "\u{feff}{\n // c\n \"a\": [1, 2,], /* d */ \"b\": { \"c\": true, },\n}";
        let value = parse_jsonc(text).unwrap();
        assert_eq!(
            value.get("a"),
            Some(&Json::Array(vec![Json::Number(1.0), Json::Number(2.0)]))
        );
        assert_eq!(value.get("b").unwrap().get("c"), Some(&Json::Bool(true)));
    }

    #[test]
    fn jsonc_reads_string_escapes_and_a_slash_inside_a_string_is_not_a_comment() {
        let value = parse_jsonc(r#"{ "p": "a//b", "q": "x\n\u00e9\ud83d\ude00" }"#).unwrap();
        assert_eq!(value.get("p").and_then(Json::as_str), Some("a//b"));
        assert_eq!(
            value.get("q").and_then(Json::as_str),
            Some("x\n\u{e9}\u{1f600}")
        );
    }

    #[test]
    fn jsonc_rejects_bad_input_with_a_line_number() {
        for bad in [
            "",
            "{",
            "{\"a\" 1}",
            "[1 2]",
            "{\"a\": }",
            "/* open",
            "{} x",
            "{'a':1}",
        ] {
            assert!(parse_jsonc(bad).is_err(), "{bad:?}");
        }
        let error = parse_jsonc("{\n\n\"a\": ?}").unwrap_err();
        assert!(error.contains("line 3"), "{error}");
    }

    #[test]
    fn jsonc_stops_at_a_depth_limit_instead_of_overflowing_the_stack() {
        let deep = "[".repeat(10_000);
        assert!(parse_jsonc(&deep).is_err());
    }

    #[test]
    fn a_glob_segment_matches_stars_and_question_marks() {
        assert!(glob_segment("*.ts", "a.ts"));
        assert!(glob_segment("a*b*c", "aXXbYYc"));
        assert!(glob_segment("a?c", "abc"));
        assert!(!glob_segment("a?c", "ac"));
        assert!(!glob_segment("*.ts", "a.tsx"));
        assert!(glob_segment("*", ""));
    }

    #[test]
    fn extension_priority_keeps_ts_over_tsx_over_dts() {
        let kept = keep_highest_extension(vec![
            PathBuf::from("/p/a.d.ts"),
            PathBuf::from("/p/a.tsx"),
            PathBuf::from("/p/a.ts"),
            PathBuf::from("/p/b.d.ts"),
            PathBuf::from("/p/c.mts"),
            PathBuf::from("/p/c.ts"),
        ]);
        assert_eq!(
            kept,
            vec![
                PathBuf::from("/p/a.ts"),
                PathBuf::from("/p/b.d.ts"),
                PathBuf::from("/p/c.mts"),
                PathBuf::from("/p/c.ts"),
            ]
        );
    }

    #[test]
    fn normalize_resolves_dots_without_the_disk() {
        assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    }
}
