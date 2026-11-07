//! The import scan of a dependency file, kept in the library cache.
//!
//! Finding what a file imports means parsing all of it, and the parse is most of what building
//! the module graph costs once the checking is cached: for a project that uses React it is
//! about 70% of the time, and `lib.dom.d.ts` alone is a third of that. What a scan returns is
//! a function of the file's bytes and its file name (the extension picks the syntax), so the
//! result for a bundled library file or a file under `node_modules` can be stored under a
//! hash of those and read back by any later run, in any project, whose copy is the same.
//!
//! Only dependency files are kept: a project's own files change all the time, and a stored
//! scan of one would be written, read once and left to pile up. The key also holds the
//! identity of the running program, as the unit entries do, because the scan depends on code
//! (the parser, the directive rules and the nesting limit) that a rebuild may change.
//!
//! The byte form is read strictly: a wrong tag or version, a length past the end, an unknown
//! kind or a trailing byte is a miss, and the file is scanned as usual. Nothing here can make
//! a scan different from the one a fresh parse gives, only skip the parse.

use std::path::Path;

use xxhash_rust::xxh3::Xxh3;

use super::discovery::{ModuleRequest, ModuleRequestKind, ModuleScan};
use super::library_cache::{LibraryUnitCache, installed};

/// Bump this when the byte form below changes.
const FORMAT: u8 = 1;
const TAG: &[u8; 4] = b"tsxS";

/// The cache to use for `path`, when it may keep a scan of it.
pub(super) fn cache_for(path: &Path) -> Option<&'static dyn LibraryUnitCache> {
    let cache = installed()?;
    let in_dependency = path
        .components()
        .any(|component| component.as_os_str() == "node_modules")
        || path.starts_with(tsox_base::standard_lib::library_directory());
    in_dependency.then_some(cache)
}

/// The key of a scan: the program, the file name and every byte of the file.
pub(super) fn key(cache: &dyn LibraryUnitCache, path: &Path, bytes: &[u8]) -> u128 {
    let mut hasher = Xxh3::new();
    hasher.update(TAG);
    hasher.update(&[FORMAT]);
    hasher.update(&cache.fingerprint().to_le_bytes());
    let name = path
        .file_name()
        .map(|name| name.as_encoded_bytes())
        .unwrap_or_default();
    hasher.update(&(name.len() as u64).to_le_bytes());
    hasher.update(name);
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
    hasher.digest128()
}

pub(super) fn encode(scan: &ModuleScan) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + scan.requests.len() * 24);
    out.extend_from_slice(TAG);
    out.push(FORMAT);
    out.push(u8::from(scan.parse_failed));
    out.push(u8::from(scan.declares_ambient_modules));
    out.extend_from_slice(&(scan.requests.len() as u32).to_le_bytes());
    for request in &scan.requests {
        out.extend_from_slice(&(request.specifier.len() as u32).to_le_bytes());
        out.extend_from_slice(request.specifier.as_bytes());
        out.push(u8::from(request.is_type));
        out.push(u8::from(request.is_import));
        out.push(kind_to_byte(request.kind));
        out.extend_from_slice(&request.start.to_le_bytes());
    }
    out
}

pub(super) fn decode(bytes: &[u8]) -> Option<ModuleScan> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.take(4)? != TAG || reader.byte()? != FORMAT {
        return None;
    }
    let parse_failed = reader.flag()?;
    let declares_ambient_modules = reader.flag()?;
    let count = reader.u32()? as usize;
    // Every request takes at least 11 bytes, so a count the rest cannot hold is damage, and
    // is refused before anything is allocated for it.
    if count > bytes.len() / 11 {
        return None;
    }
    let mut requests = Vec::with_capacity(count);
    for _ in 0..count {
        let length = reader.u32()? as usize;
        let specifier = std::str::from_utf8(reader.take(length)?).ok()?.to_string();
        let is_type = reader.flag()?;
        let is_import = reader.flag()?;
        let kind = kind_from_byte(reader.byte()?)?;
        let start = reader.u32()?;
        requests.push(ModuleRequest {
            specifier,
            is_type,
            is_import,
            kind,
            start,
        });
    }
    (reader.at == bytes.len()).then_some(ModuleScan {
        requests,
        parse_failed,
        declares_ambient_modules,
    })
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(count)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn flag(&mut self) -> Option<bool> {
        match self.byte()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
}

// Exhaustive on both sides, so a new kind is a compile error here and not a scan that
// silently round-trips as another kind.
fn kind_to_byte(kind: ModuleRequestKind) -> u8 {
    match kind {
        ModuleRequestKind::StaticImport => 0,
        ModuleRequestKind::TypeImport => 1,
        ModuleRequestKind::SideEffectImport => 2,
        ModuleRequestKind::ReExport => 3,
        ModuleRequestKind::DynamicImport => 4,
        ModuleRequestKind::ImportEquals => 5,
        ModuleRequestKind::ModuleAugmentation => 6,
        ModuleRequestKind::LibraryReference => 7,
    }
}

fn kind_from_byte(byte: u8) -> Option<ModuleRequestKind> {
    Some(match byte {
        0 => ModuleRequestKind::StaticImport,
        1 => ModuleRequestKind::TypeImport,
        2 => ModuleRequestKind::SideEffectImport,
        3 => ModuleRequestKind::ReExport,
        4 => ModuleRequestKind::DynamicImport,
        5 => ModuleRequestKind::ImportEquals,
        6 => ModuleRequestKind::ModuleAugmentation,
        7 => ModuleRequestKind::LibraryReference,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module_resolution::scan_module_requests;

    fn sample() -> ModuleScan {
        scan_module_requests(
            "/// <reference lib=\"es2015\" />\n/// <reference path=\"./b.d.ts\" />\n\
             import type { A } from './a';\nimport './side';\nexport * from './re';\n\
             import x = require('./eq');\nconst d = import('./dyn');\n\
             declare module 'ambient' { export const n: number }\n",
            "sample.d.ts",
        )
    }

    #[test]
    fn a_scan_round_trips_exactly() {
        let scan = sample();
        assert!(scan.requests.len() >= 6, "{:?}", scan.requests);
        let back = decode(&encode(&scan)).expect("decodes");
        assert_eq!(back.requests, scan.requests);
        assert_eq!(back.parse_failed, scan.parse_failed);
        assert_eq!(back.declares_ambient_modules, scan.declares_ambient_modules);
    }

    #[test]
    fn every_kind_survives_the_byte_form() {
        for byte in 0..=u8::MAX {
            match kind_from_byte(byte) {
                Some(kind) => assert_eq!(kind_to_byte(kind), byte),
                None => assert!(byte > 7),
            }
        }
    }

    #[test]
    fn damaged_bytes_are_a_miss_not_a_panic_or_a_wrong_scan() {
        let bytes = encode(&sample());
        // Every strict prefix, and one extra byte.
        for end in 0..bytes.len() {
            assert!(decode(&bytes[..end]).is_none(), "prefix of {end} bytes");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode(&longer).is_none());
        // Another version, another tag, a flag that is neither 0 nor 1, an unknown kind.
        let mut other = bytes.clone();
        other[4] = FORMAT + 1;
        assert!(decode(&other).is_none());
        let mut other = bytes.clone();
        other[0] ^= 0xff;
        assert!(decode(&other).is_none());
        let mut other = bytes.clone();
        other[5] = 2;
        assert!(decode(&other).is_none());
        // A count far past what the bytes could hold is refused without allocating for it.
        let mut huge = bytes.clone();
        huge[7..11].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&huge).is_none());
        assert!(decode(&[]).is_none());
    }

    #[test]
    fn the_key_changes_with_the_name_and_with_every_byte() {
        struct Fixed;
        impl LibraryUnitCache for Fixed {
            fn load(&self, _: u128) -> Option<Vec<u8>> {
                None
            }
            fn store(&self, _: u128, _: &[u8]) {}
            fn fingerprint(&self) -> u128 {
                7
            }
        }
        let base = key(&Fixed, Path::new("/a/index.d.ts"), b"export {};");
        assert_eq!(
            base,
            key(&Fixed, Path::new("/elsewhere/index.d.ts"), b"export {};")
        );
        assert_ne!(base, key(&Fixed, Path::new("/a/index.ts"), b"export {};"));
        assert_ne!(
            base,
            key(&Fixed, Path::new("/a/index.d.ts"), b"export {}; ")
        );
    }
}
