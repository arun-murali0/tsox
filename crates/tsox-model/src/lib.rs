// Problem: what the checker knows about a file (the type of a node, what a name refers to,
// the narrowed type at a read, constant values) exists only while the file's arena is alive.
// Emit, the language server and any IR would each have to re-derive it or reach into the
// checker's internals.
// Picked: one arena-free, offset-free, versioned set of tables that the checker writes once
// and every consumer reads. This crate sits between `tsox-types` and `tsox-checker`: it names
// no AST and no checker type, so a consumer can depend on it without depending on how the
// facts were found.
// Cost: each table has one writer in the checker (`UnitOutput.model`), but the coverage is
// partial: types are recorded for exported declarations only, symbols for references to exported
// declarations, flow facts for reads the flow walk answered. The gaps are listed in
// docs/architecture/new-architecture-progress.md.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

use tsox_base::keys::DeclKey;
use tsox_types::portable::PortableType;

/// Bumped whenever the shape of a table or its hashing changes, so a stored model from an
/// older build is never read as a current one. It is part of every model hash.
pub const MODEL_FORMAT_VERSION: u32 = 1;

/// Names a node without a byte offset: the item it sits in (a hash of its path of names,
/// never an AST-order index) and the node's place within that item. A comment or an edit
/// above the item changes neither.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeKey {
    pub item: u64,
    pub index: u32,
}

/// Whether a value's type may be relied on without a run-time check. TypeScript types are not
/// sound (`any`, assertions, structural subtyping), so an IR starts from `Trusted` only where
/// the checker is sure.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Trust {
    Trusted,
    Unchecked,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TypeFact {
    pub ty: PortableType,
    pub trust: Trust,
}

/// A value known at check time: what `const enum` inlining and constant folding read.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ConstValue {
    /// The bits of the `f64` (see `ConstValue::number`), so the value hashes and compares exactly.
    Number(u64),
    String(Box<str>),
    Boolean(bool),
}

impl ConstValue {
    /// A number; every NaN is one value so equal models hash equal.
    pub fn number(value: f64) -> Self {
        Self::Number(if value.is_nan() { f64::NAN } else { value }.to_bits())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(bits) => Some(f64::from_bits(*bits)),
            _ => None,
        }
    }
}

/// The key of one enum member: the enum's name and the member's, in the module. A member is
/// not an export, so the key is of the enum kind with a dotted name no declaration has. The
/// checker writes `const_values` under it and emit reads them back by it.
pub fn enum_member_key(
    module: tsox_base::keys::ModuleKey,
    enum_name: &str,
    member: &str,
) -> DeclKey {
    DeclKey::new(
        module,
        &format!("{enum_name}.{member}"),
        tsox_base::keys::DeclKind::Enum,
        0,
    )
}

/// How a module's imports are used, for import elision.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ImportUse {
    Value,
    TypeOnly,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModuleInfo {
    /// Exported names, sorted.
    pub exports: Vec<(Box<str>, DeclKey)>,
    /// Each import specifier with how it is used, sorted by specifier.
    pub imports: Vec<(Box<str>, ImportUse)>,
}

/// The facts about one checked file. Every table is ordered, so iteration and the hash do not
/// depend on the order the checker produced them in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SemanticModel {
    /// The type of an expression or declaration node.
    pub node_type: BTreeMap<NodeKey, TypeFact>,
    /// What an identifier node refers to.
    pub node_symbol: BTreeMap<NodeKey, DeclKey>,
    /// The narrowed type of a reference at its read.
    pub flow_facts: BTreeMap<NodeKey, TypeFact>,
    /// The value of a constant declaration.
    pub const_values: BTreeMap<DeclKey, ConstValue>,
    pub module_info: ModuleInfo,
}

impl SemanticModel {
    pub fn is_empty(&self) -> bool {
        self.node_type.is_empty()
            && self.node_symbol.is_empty()
            && self.flow_facts.is_empty()
            && self.const_values.is_empty()
            && self.module_info == ModuleInfo::default()
    }

    /// A hash a query can cut off on: equal models give equal hashes in any run, and a change
    /// of `MODEL_FORMAT_VERSION` changes every hash.
    pub fn stable_hash(&self) -> u64 {
        let mut hasher = xxhash_rust::xxh3::Xxh3::new();
        MODEL_FORMAT_VERSION.hash(&mut hasher);
        for (key, fact) in &self.node_type {
            (0u8, key, fact).hash(&mut hasher);
        }
        for (key, symbol) in &self.node_symbol {
            (1u8, key, symbol).hash(&mut hasher);
        }
        for (key, fact) in &self.flow_facts {
            (2u8, key, fact).hash(&mut hasher);
        }
        for (key, value) in &self.const_values {
            (3u8, key, value).hash(&mut hasher);
        }
        (4u8, &self.module_info).hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_model_is_empty_and_hashes_the_same_every_time() {
        let a = SemanticModel::default();
        let b = SemanticModel::default();
        assert!(a.is_empty());
        assert_eq!(a.stable_hash(), b.stable_hash());
    }

    #[test]
    fn a_fact_changes_the_hash_and_insertion_order_does_not() {
        let key = |index| NodeKey { item: 7, index };
        let mut one = SemanticModel::default();
        one.const_values
            .insert(DeclKey::builtin("A"), ConstValue::Boolean(true));
        assert!(!one.is_empty());
        assert_ne!(one.stable_hash(), SemanticModel::default().stable_hash());

        let (mut a, mut b) = (SemanticModel::default(), SemanticModel::default());
        a.node_symbol.insert(key(1), DeclKey::builtin("A"));
        a.node_symbol.insert(key(2), DeclKey::builtin("B"));
        b.node_symbol.insert(key(2), DeclKey::builtin("B"));
        b.node_symbol.insert(key(1), DeclKey::builtin("A"));
        assert_eq!(a.stable_hash(), b.stable_hash());
    }
}
