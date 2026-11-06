// Stable identity for modules and declarations (LLD 3.1, 3.2).
//
// Every key is XXH3-128 over an explicit byte stream and never #[derive(Hash)], whose
// layout is not a stable format. The first byte of every stream is ENC_VERSION, so a
// change to any encoding bumps it and old fingerprints stop matching instead of mixing
// formats. Nothing run-local goes in: no FileId, no TypeId, no absolute project root,
// no modification time.

use std::path::{Component, Path};

use xxhash_rust::xxh3::Xxh3;

pub const ENC_VERSION: u8 = 3;

macro_rules! key128 {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[cfg_attr(feature = "snapshot", derive(serde::Serialize, serde::Deserialize))]
        #[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(u128);

        impl $name {
            pub const fn from_u128(bits: u128) -> Self {
                Self(bits)
            }
            pub const fn as_u128(self) -> u128 {
                self.0
            }
            pub const fn to_le_bytes(self) -> [u8; 16] {
                self.0.to_le_bytes()
            }
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, concat!(stringify!($name), "({:032x})"), self.0)
            }
        }
    };
}

key128!(
    /// Identity of a module, by the rule in LLD 3.1.1. Survives moving the project.
    ModuleKey
);
key128!(
    /// Identity of one declaration: its module, local name and merge-group position.
    DeclKey
);
key128!(
    /// Hash of a declaration's own public shape (LLD 3.3). Other declarations appear
    /// in it by `DeclKey` only, so a body-only edit leaves it unchanged.
    SigHash
);
key128!(
    /// Merkle hash of one type node (LLD 3.2).
    NodeHash
);
key128!(
    /// Identity of an import cycle: the keys of its members, sorted, hashed together.
    CycleKey
);

// The byte stream a hash is computed over. Integers are little endian, strings and
// byte runs carry a u32 length first, so no two field layouts can read the same.
pub struct StreamWriter(Xxh3);

impl Default for StreamWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamWriter {
    pub fn new() -> Self {
        let mut w = Self(Xxh3::new());
        w.u8(ENC_VERSION);
        w
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.0.update(&[v]);
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }
    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.u32(u32::try_from(b.len()).expect("a hashed field is under 4 GiB"));
        self.0.update(b);
        self
    }
    pub fn str(&mut self, s: &str) -> &mut Self {
        self.bytes(s.as_bytes())
    }
    pub fn raw128(&mut self, v: u128) -> &mut Self {
        self.0.update(&v.to_le_bytes());
        self
    }
    pub fn finish(&self) -> u128 {
        self.0.digest128()
    }
}

// Module kind bytes (LLD 3.1.1). 0x04 is not in the LLD: it names the built-in library
// declarations (`Promise`, `Array` ...) so they have a DeclKey without a file.
const KIND_PROJECT: u8 = 0x01;
const KIND_PACKAGE: u8 = 0x02;
const KIND_EXTERNAL: u8 = 0x03;
const KIND_BUILTIN: u8 = 0x04;
const KIND_AMBIENT: u8 = 0x05;
const KIND_AMBIENT_CHILD: u8 = 0x06;

impl ModuleKey {
    /// `path` is relative to the project root, with `/` separators.
    pub fn project(path: &str) -> Self {
        let mut w = StreamWriter::new();
        w.u8(KIND_PROJECT).str(path);
        Self(w.finish())
    }

    /// `name` is verbatim, scope included. `subpath` is relative to the package
    /// directory, with `/` separators. A package is named for what it is, not where it
    /// was installed, so pnpm's layout and a moved project give the same key.
    pub fn package(name: &str, version: &str, subpath: &str) -> Self {
        let mut w = StreamWriter::new();
        w.u8(KIND_PACKAGE).str(name).str(version).str(subpath);
        Self(w.finish())
    }

    /// `path` is the absolute canonical path, with `/` separators.
    pub fn external(path: &str) -> Self {
        let mut w = StreamWriter::new();
        w.u8(KIND_EXTERNAL).str(path);
        Self(w.finish())
    }

    /// A module that exists only as a `declare module "specifier" { ... }` block: it is
    /// named by the specifier it answers, wherever the block was written.
    pub fn ambient(specifier: &str) -> Self {
        let mut w = StreamWriter::new();
        w.u8(KIND_AMBIENT).str(specifier);
        Self(w.finish())
    }

    /// A block that declares or augments an ambient module. The host module and the
    /// deterministic ordinal distinguish repeated `declare module "x"` blocks, including
    /// declarations of the same name in separate files. The ordinal is counted per
    /// specifier within one file, so adding an unrelated declaration does not renumber it.
    pub fn ambient_child(specifier: &str, owner: ModuleKey, ordinal: u32) -> Self {
        let mut w = StreamWriter::new();
        w.u8(KIND_AMBIENT_CHILD)
            .str(specifier)
            .raw128(owner.as_u128())
            .u32(ordinal);
        Self(w.finish())
    }

    /// The module of the built-in library declarations.
    pub fn builtin() -> Self {
        let mut w = StreamWriter::new();
        w.u8(KIND_BUILTIN);
        Self(w.finish())
    }

    /// The `project` key for `file` if it lies under `root` and does not pass through
    /// a `node_modules` directory; None otherwise (the caller then looks for a package
    /// or falls back to `external`). Purely lexical: no filesystem access, so it works
    /// for in-memory and wasm inputs. `.` components are dropped, `..` is refused.
    pub fn project_path(root: &Path, file: &Path) -> Option<String> {
        let rel = file.strip_prefix(root).ok()?;
        let mut parts: Vec<&str> = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => {
                    let s = s.to_str()?;
                    if s == "node_modules" {
                        return None;
                    }
                    parts.push(s);
                }
                Component::CurDir => {}
                _ => return None,
            }
        }
        if parts.is_empty() {
            return None;
        }
        Some(parts.join("/"))
    }
}

/// What a declaration is, for the merge-group disambiguator. The tag values are part of
/// the encoding: never renumber one, bump `ENC_VERSION` instead.
#[cfg_attr(feature = "snapshot", derive(serde::Serialize, serde::Deserialize))]
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum DeclKind {
    Interface = 1,
    Class = 2,
    Alias = 3,
    Function = 4,
    Var = 5,
    Enum = 6,
    Namespace = 7,
}

/// The TypeScript symbol spaces contributed by a declaration.
///
/// TypeScript has separate type and value namespaces. A declaration may contribute to
/// either namespace or both: interfaces and aliases are type-only, functions and
/// variables are value-only, and classes/enums contribute to both. Keeping this mapping
/// next to `DeclKind` gives every importer and global-registration path one shared rule
/// instead of reconstructing it with slightly different variant matches.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SymbolSpaces(u8);

impl SymbolSpaces {
    pub const TYPE: Self = Self(0b01);
    pub const VALUE: Self = Self(0b10);

    pub const fn contains(self, space: Self) -> bool {
        self.0 & space.0 == space.0
    }
}

impl DeclKind {
    /// Which namespaces this declaration contributes to in TypeScript.
    pub const fn symbol_spaces(self) -> SymbolSpaces {
        match self {
            Self::Interface | Self::Alias => SymbolSpaces::TYPE,
            Self::Class | Self::Enum => SymbolSpaces(SymbolSpaces::TYPE.0 | SymbolSpaces::VALUE.0),
            Self::Function | Self::Var | Self::Namespace => SymbolSpaces::VALUE,
        }
    }
}

impl DeclKey {
    /// `index` is the position in source order among declarations of the same kind
    /// and name in this module (overloads, merged declarations), so each is distinct
    /// without the key depending on a span.
    pub fn new(module: ModuleKey, name: &str, kind: DeclKind, index: u32) -> Self {
        let mut w = StreamWriter::new();
        w.u8(0xDE)
            .raw128(module.as_u128())
            .str(name)
            .u8(kind as u8)
            .u32(index);
        Self(w.finish())
    }

    /// A declaration of the built-in library.
    pub fn builtin(name: &str) -> Self {
        Self::new(ModuleKey::builtin(), name, DeclKind::Interface, 0)
    }

    /// A key for something that owns type parameters but is not itself a declaration,
    /// such as the second generic function type inside one alias. `ordinal` counts
    /// those in source order within `self`. This keeps `TypeParam(owner, index)` unique
    /// without changing the wire format.
    pub fn child(self, ordinal: u32) -> Self {
        let mut w = StreamWriter::new();
        w.u8(0xDC).raw128(self.as_u128()).u32(ordinal);
        Self(w.finish())
    }

    /// Stable owner for a generic signature nested inside a declaration (a method of an
    /// interface, a callback parameter, a function typed property). It is the key of the
    /// declaration it is written in plus the path from that declaration's type down to the
    /// signature, so it does not change when lines are added above it: an offset in the
    /// file would, and a hash that moved with it would tell a dependent that an unchanged
    /// signature had changed.
    pub fn nested_type_signature(parent: DeclKey, path: &[u8]) -> Self {
        let mut w = StreamWriter::new();
        w.u8(0xDD).raw128(parent.as_u128()).u32(path.len() as u32);
        for byte in path {
            w.u8(*byte);
        }
        Self(w.finish())
    }
}

impl CycleKey {
    /// The key of the cycle whose members have these module keys. The order they are given
    /// in does not matter and a repeated key counts once, so the same set of modules gives
    /// the same key wherever the project is checked out and however the files were found.
    pub fn of(members: &[ModuleKey]) -> Self {
        let mut sorted: Vec<u128> = members.iter().map(|member| member.as_u128()).collect();
        sorted.sort_unstable();
        sorted.dedup();
        let mut w = StreamWriter::new();
        w.u8(0xDE)
            .u32(u32::try_from(sorted.len()).expect("fewer than 2^32 members"));
        for member in sorted {
            w.raw128(member);
        }
        Self(w.finish())
    }
}

/// A type parameter's stable identity: the declaration (or `child` of one) that
/// declares it, and its position in that list.
#[cfg_attr(feature = "snapshot", derive(serde::Serialize, serde::Deserialize))]
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct TypeParamKey {
    pub owner: DeclKey,
    pub index: u32,
}

#[cfg(test)]
mod symbol_space_tests {
    use super::{DeclKind, SymbolSpaces};

    #[test]
    fn declaration_kinds_contribute_to_the_correct_symbol_spaces() {
        let has = |kind: DeclKind, space: SymbolSpaces| kind.symbol_spaces().contains(space);

        assert!(has(DeclKind::Interface, SymbolSpaces::TYPE));
        assert!(!has(DeclKind::Interface, SymbolSpaces::VALUE));
        assert!(has(DeclKind::Alias, SymbolSpaces::TYPE));
        assert!(!has(DeclKind::Alias, SymbolSpaces::VALUE));
        assert!(has(DeclKind::Class, SymbolSpaces::TYPE));
        assert!(has(DeclKind::Class, SymbolSpaces::VALUE));
        assert!(has(DeclKind::Enum, SymbolSpaces::TYPE));
        assert!(has(DeclKind::Enum, SymbolSpaces::VALUE));
        assert!(!has(DeclKind::Function, SymbolSpaces::TYPE));
        assert!(has(DeclKind::Function, SymbolSpaces::VALUE));
        assert!(!has(DeclKind::Var, SymbolSpaces::TYPE));
        assert!(has(DeclKind::Var, SymbolSpaces::VALUE));
        assert!(!has(DeclKind::Namespace, SymbolSpaces::TYPE));
        assert!(has(DeclKind::Namespace, SymbolSpaces::VALUE));
    }
}
