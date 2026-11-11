//! What a unit asked of the rest of the project, and what it was told.
//!
//! A unit's output is a function of its text, its options and the answers it got from its
//! `ImportSource`: nothing else reaches the checker. [`RecordingImports`] sits between a unit
//! and the real source, passes every question through and keeps each distinct question with a
//! fingerprint of the answer. Later, [`ProbeRecord::holds`] asks the same questions of the
//! project as it is then, and a cached output may be used only if every fingerprint matches.
//! That makes a stored output exactly the one a fresh check would produce, whatever else
//! changed in the project, including questions answered with "nothing" (a name that no file
//! declared when the output was made, and one does now).
//!
//! Every method of `ImportSource` and `DeclProvider` must be forwarded and recorded here. A
//! method added to the trait without being added below would fall back to the trait's default
//! and go unrecorded, so `tests/probe_log.rs` lists them and fails when the two drift.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::{Arc, Mutex};

use tsox_base::keys::DeclKey;
use tsox_types::portable::{
    DeclProvider, GlobalNamespace, ImportSource, ModuleExports, PortableDecl,
};
use xxhash_rust::xxh3::Xxh3;

/// One question to an `ImportSource`, with its arguments.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ProbeCall {
    Decl(DeclKey),
    ExportKeys(String, String),
    ModuleExports(String),
    ExportAll(String),
    GlobalNamespaces,
    GlobalDecls(String),
    RequiresRegisteredGlobalTypes,
    RequiresRegisteredGlobalValues,
    CollectGlobalDecls,
    GlobalNameUnmodelled(String),
    GlobalGenericParams(String),
    GlobalTypeOnlyUnmodelled(String),
    GlobalMembers(String),
    GlobalNameIsConst(String),
    ExportPrefixed(String, String),
}

/// A question and a fingerprint of the answer it got.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProbeRecord {
    pub call: ProbeCall,
    pub answer: u128,
}

impl ProbeRecord {
    /// True when `source` gives the same answer to this question now. `memo` saves hashing a
    /// declaration that has been hashed already in this run.
    pub fn holds(&self, source: &dyn ImportSource, memo: Option<&FingerprintMemo>) -> bool {
        answer_of(&self.call, source, memo) == self.answer
    }
}

/// True when every record still holds.
pub fn all_hold(
    records: &[ProbeRecord],
    source: &dyn ImportSource,
    memo: Option<&FingerprintMemo>,
) -> bool {
    records.iter().all(|record| record.holds(source, memo))
}

/// The fingerprints of declarations already hashed, so a declaration that many units ask for
/// (`Array`, `Promise`, an HTML element) is hashed once and not once per unit.
///
/// A declaration is told apart by the address of its `Arc`, and the memo keeps a strong
/// reference to every `Arc` it has an entry for. While the entry exists the allocation cannot
/// be freed, so its address cannot be handed to another declaration, and a declaration behind
/// an `Arc` is never changed. A hit is therefore always the same declaration and the same
/// content, and the hash is the one a fresh computation would give. Make one per run: it holds
/// what it has seen until it is dropped.
#[derive(Default)]
pub struct FingerprintMemo {
    seen: std::sync::RwLock<HashMap<usize, (Arc<PortableDecl>, u128)>>,
}

impl FingerprintMemo {
    pub fn new() -> Self {
        Self::default()
    }

    fn decl(&self, decl: &Arc<PortableDecl>) -> u128 {
        let address = Arc::as_ptr(decl) as usize;
        if let Some((_, fingerprint)) = self
            .seen
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&address)
        {
            return *fingerprint;
        }
        // Hashed outside the lock. Two threads may both do it; they get the same value.
        let fingerprint = fp_decl(Some(decl));
        self.seen
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(address, (Arc::clone(decl), fingerprint));
        fingerprint
    }

    /// How many declarations have been hashed.
    pub fn len(&self) -> usize {
        self.seen
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn fp_decl_memo(decl: Option<Arc<PortableDecl>>, memo: Option<&FingerprintMemo>) -> u128 {
    match (decl, memo) {
        (Some(decl), Some(memo)) => memo.decl(&decl),
        (decl, _) => fp_decl(decl.as_deref()),
    }
}

/// The declarations the questions asked for, sorted and without duplicates: what
/// `FileReport::reads` holds for a check that asked them.
pub fn declarations_read(records: &[ProbeRecord]) -> Vec<DeclKey> {
    let mut keys: Vec<DeclKey> = records
        .iter()
        .filter_map(|record| match &record.call {
            ProbeCall::Decl(key) => Some(*key),
            _ => None,
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

fn answer_of(call: &ProbeCall, source: &dyn ImportSource, memo: Option<&FingerprintMemo>) -> u128 {
    match call {
        ProbeCall::Decl(key) => fp_decl_memo(source.decl(*key), memo),
        ProbeCall::ExportKeys(specifier, name) => fp_keys(&source.export_keys(specifier, name)),
        ProbeCall::ModuleExports(specifier) => fp_module_exports(source.module_exports(specifier)),
        ProbeCall::ExportAll(specifier) => fp_pairs(&source.export_all(specifier)),
        ProbeCall::ExportPrefixed(specifier, prefix) => {
            fp_pairs(&source.export_prefixed(specifier, prefix))
        }
        ProbeCall::GlobalNamespaces => fp_namespaces(&source.global_namespaces()),
        ProbeCall::GlobalDecls(name) => fp_keys(&source.global_decls(name)),
        ProbeCall::RequiresRegisteredGlobalTypes => {
            fp_bool(source.requires_registered_global_types())
        }
        ProbeCall::RequiresRegisteredGlobalValues => {
            fp_bool(source.requires_registered_global_values())
        }
        ProbeCall::CollectGlobalDecls => fp_bool(source.collect_global_decls()),
        ProbeCall::GlobalNameUnmodelled(name) => fp_bool(source.global_name_unmodelled(name)),
        ProbeCall::GlobalGenericParams(name) => fp_generics(source.global_generic_params(name)),
        ProbeCall::GlobalTypeOnlyUnmodelled(name) => {
            fp_bool(source.global_type_only_unmodelled(name))
        }
        ProbeCall::GlobalMembers(root) => fp_strings(&source.global_members(root)),
        ProbeCall::GlobalNameIsConst(name) => fp_bool(source.global_name_is_const(name)),
    }
}

// -- fingerprints ---------------------------------------------------------------------
//
// One hasher per answer, fed the whole answer: a declaration's full content, since a unit
// can print its names and read its parameters, and not only its `SigHash`.

fn fp_decl(decl: Option<&PortableDecl>) -> u128 {
    let mut hasher = Xxh3::new();
    decl.hash(&mut hasher);
    hasher.digest128()
}

fn fp_keys(keys: &[DeclKey]) -> u128 {
    let mut hasher = Xxh3::new();
    keys.hash(&mut hasher);
    hasher.digest128()
}

fn fp_pairs(pairs: &[(String, DeclKey)]) -> u128 {
    let mut hasher = Xxh3::new();
    pairs.hash(&mut hasher);
    hasher.digest128()
}

fn fp_module_exports(exports: ModuleExports) -> u128 {
    let mut hasher = Xxh3::new();
    match exports {
        ModuleExports::Unresolved => 0u8.hash(&mut hasher),
        ModuleExports::Unknown => 1u8.hash(&mut hasher),
        ModuleExports::Known { complete } => (2u8, complete).hash(&mut hasher),
    }
    hasher.digest128()
}

fn fp_namespaces(namespaces: &[GlobalNamespace]) -> u128 {
    let mut hasher = Xxh3::new();
    namespaces.len().hash(&mut hasher);
    for namespace in namespaces {
        namespace.name.hash(&mut hasher);
        namespace.members.hash(&mut hasher);
        namespace.export_assignment.hash(&mut hasher);
    }
    hasher.digest128()
}

fn fp_strings(values: &[String]) -> u128 {
    let mut hasher = Xxh3::new();
    values.hash(&mut hasher);
    hasher.digest128()
}

fn fp_bool(value: bool) -> u128 {
    u128::from(value)
}

fn fp_generics(value: Option<Vec<(String, bool)>>) -> u128 {
    let mut hasher = Xxh3::new();
    value.hash(&mut hasher);
    hasher.digest128()
}

// -- recording -------------------------------------------------------------------------

/// An `ImportSource` that forwards to another and keeps each distinct question it was asked.
pub struct RecordingImports<'a> {
    inner: &'a dyn ImportSource,
    memo: Option<&'a FingerprintMemo>,
    log: Mutex<Log>,
}

#[derive(Default)]
struct Log {
    seen: HashSet<ProbeCall>,
    records: Vec<ProbeRecord>,
}

impl<'a> RecordingImports<'a> {
    pub fn new(inner: &'a dyn ImportSource) -> Self {
        Self {
            inner,
            memo: None,
            log: Mutex::new(Log::default()),
        }
    }

    /// Like `new`, hashing declarations through `memo`.
    pub fn with_memo(inner: &'a dyn ImportSource, memo: &'a FingerprintMemo) -> Self {
        Self {
            memo: Some(memo),
            ..Self::new(inner)
        }
    }

    /// The questions asked so far, in the order they were first asked.
    pub fn into_records(self) -> Vec<ProbeRecord> {
        self.log
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .records
    }

    // Records `call` the first time it is asked. The answer is fingerprinted only then, so a
    // question asked a hundred times costs one hash; within one unit run the project does not
    // change under it.
    fn note(&self, call: impl FnOnce() -> ProbeCall, answer: impl FnOnce() -> u128) {
        let call = call();
        let mut log = self
            .log
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if log.seen.contains(&call) {
            return;
        }
        log.seen.insert(call.clone());
        log.records.push(ProbeRecord {
            call,
            answer: answer(),
        });
    }
}

impl DeclProvider for RecordingImports<'_> {
    fn decl(&self, key: DeclKey) -> Option<Arc<PortableDecl>> {
        let answer = self.inner.decl(key);
        self.note(
            || ProbeCall::Decl(key),
            || fp_decl_memo(answer.clone(), self.memo),
        );
        answer
    }
}

impl ImportSource for RecordingImports<'_> {
    fn export_keys(&self, specifier: &str, name: &str) -> Vec<DeclKey> {
        let answer = self.inner.export_keys(specifier, name);
        self.note(
            || ProbeCall::ExportKeys(specifier.to_string(), name.to_string()),
            || fp_keys(&answer),
        );
        answer
    }

    fn module_exports(&self, specifier: &str) -> ModuleExports {
        let answer = self.inner.module_exports(specifier);
        self.note(
            || ProbeCall::ModuleExports(specifier.to_string()),
            || fp_module_exports(answer),
        );
        answer
    }

    fn export_all(&self, specifier: &str) -> Vec<(String, DeclKey)> {
        let answer = self.inner.export_all(specifier);
        self.note(
            || ProbeCall::ExportAll(specifier.to_string()),
            || fp_pairs(&answer),
        );
        answer
    }

    fn export_prefixed(&self, specifier: &str, prefix: &str) -> Vec<(String, DeclKey)> {
        let answer = self.inner.export_prefixed(specifier, prefix);
        self.note(
            || ProbeCall::ExportPrefixed(specifier.to_string(), prefix.to_string()),
            || fp_pairs(&answer),
        );
        answer
    }

    fn global_namespaces(&self) -> Vec<GlobalNamespace> {
        let answer = self.inner.global_namespaces();
        self.note(|| ProbeCall::GlobalNamespaces, || fp_namespaces(&answer));
        answer
    }

    fn global_decls(&self, name: &str) -> Vec<DeclKey> {
        let answer = self.inner.global_decls(name);
        self.note(
            || ProbeCall::GlobalDecls(name.to_string()),
            || fp_keys(&answer),
        );
        answer
    }

    fn requires_registered_global_types(&self) -> bool {
        let answer = self.inner.requires_registered_global_types();
        self.note(
            || ProbeCall::RequiresRegisteredGlobalTypes,
            || fp_bool(answer),
        );
        answer
    }

    fn requires_registered_global_values(&self) -> bool {
        let answer = self.inner.requires_registered_global_values();
        self.note(
            || ProbeCall::RequiresRegisteredGlobalValues,
            || fp_bool(answer),
        );
        answer
    }

    fn collect_global_decls(&self) -> bool {
        let answer = self.inner.collect_global_decls();
        self.note(|| ProbeCall::CollectGlobalDecls, || fp_bool(answer));
        answer
    }

    fn global_name_unmodelled(&self, name: &str) -> bool {
        let answer = self.inner.global_name_unmodelled(name);
        self.note(
            || ProbeCall::GlobalNameUnmodelled(name.to_string()),
            || fp_bool(answer),
        );
        answer
    }

    fn global_generic_params(&self, name: &str) -> Option<Vec<(String, bool)>> {
        let answer = self.inner.global_generic_params(name);
        self.note(
            || ProbeCall::GlobalGenericParams(name.to_string()),
            || fp_generics(answer.clone()),
        );
        answer
    }

    fn global_type_only_unmodelled(&self, name: &str) -> bool {
        let answer = self.inner.global_type_only_unmodelled(name);
        self.note(
            || ProbeCall::GlobalTypeOnlyUnmodelled(name.to_string()),
            || fp_bool(answer),
        );
        answer
    }

    fn global_members(&self, root: &str) -> Vec<String> {
        let answer = self.inner.global_members(root);
        self.note(
            || ProbeCall::GlobalMembers(root.to_string()),
            || fp_strings(&answer),
        );
        answer
    }

    fn global_name_is_const(&self, name: &str) -> bool {
        let answer = self.inner.global_name_is_const(name);
        self.note(
            || ProbeCall::GlobalNameIsConst(name.to_string()),
            || fp_bool(answer),
        );
        answer
    }
}
