//! A byte form for what a unit hands back, so the result of checking a bundled library file
//! can be stored and read again instead of being computed on every run.
//!
//! A snapshot holds the whole of a [`UnitOutput`], diagnostics included, so a decoded one is
//! the unit that was checked and not a quieter copy of it. What it holds is arena-free data
//! (LLD 2.1 rule 3), so it is usable in any worker.
//!
//! The header carries the format number of this module and `ENC_VERSION` of the key
//! encoding. A reader that sees either differ gets an error and recomputes; it never reads
//! bytes written under another encoding.

use bincode::Options;
use thiserror::Error;
use tsox_base::diagnostics::Diagnostic;
use tsox_base::keys::{DeclKey, ENC_VERSION};
use tsox_types::portable::{ImportSource, PortableDecl};

use crate::bridge::IncompleteReason;
use crate::probe_log::{ProbeRecord, all_hold};
use crate::unit::{AmbientExports, ReturnRead, UnitOutput};

/// Bump this when the fields below, or the meaning of one, change.
pub const SNAPSHOT_FORMAT: u32 = 3;

/// Bytes a snapshot may take when decoded. A library file's output is a few hundred kilobytes;
/// the bound only stops a damaged length prefix from asking for a huge allocation.
const DECODE_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("snapshot format {found} is not the supported format {SNAPSHOT_FORMAT}")]
    Format { found: u32 },
    #[error("snapshot was written under key encoding {found}, not {ENC_VERSION}")]
    Encoding { found: u8 },
    #[error("snapshot bytes are damaged: {0}")]
    Damaged(String),
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UnitSnapshot {
    format: u32,
    enc_version: u8,
    diagnostics: Vec<Diagnostic>,
    incomplete: bool,
    incomplete_reasons: Vec<IncompleteReason>,
    exports: Vec<PortableDecl>,
    export_names: Vec<(String, DeclKey)>,
    exports_complete: bool,
    ambient: Vec<AmbientExports>,
    umd_namespaces: Vec<String>,
    unresolved_names: Vec<String>,
    augmentations: Vec<AmbientExports>,
    globals: Vec<AmbientExports>,
    return_reads: Vec<ReturnRead>,
}

impl UnitSnapshot {
    /// What `output` holds.
    pub fn capture(output: &UnitOutput) -> Self {
        Self {
            format: SNAPSHOT_FORMAT,
            enc_version: ENC_VERSION,
            diagnostics: output.diagnostics.clone(),
            incomplete: output.incomplete,
            incomplete_reasons: output.incomplete_reasons.clone(),
            exports: output.exports.clone(),
            export_names: output.export_names.clone(),
            exports_complete: output.exports_complete,
            ambient: output.ambient.clone(),
            umd_namespaces: output.umd_namespaces.clone(),
            unresolved_names: output.unresolved_names.clone(),
            augmentations: output.augmentations.clone(),
            globals: output.globals.clone(),
            return_reads: output.return_reads.clone(),
        }
    }

    /// The unit output this snapshot was taken from.
    pub fn into_output(self) -> UnitOutput {
        UnitOutput {
            diagnostics: self.diagnostics,
            incomplete: self.incomplete,
            incomplete_reasons: self.incomplete_reasons,
            exports: self.exports,
            export_names: self.export_names,
            exports_complete: self.exports_complete,
            ambient: self.ambient,
            umd_namespaces: self.umd_namespaces,
            unresolved_names: self.unresolved_names,
            augmentations: self.augmentations,
            globals: self.globals,
            return_reads: self.return_reads,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, SnapshotError> {
        options()
            .serialize(self)
            .map_err(|error| SnapshotError::Damaged(error.to_string()))
    }

    /// Reads a snapshot, refusing one written under another format or key encoding. The
    /// two header fields are read first, so a newer layout is reported as such and not as
    /// damage.
    pub fn decode(bytes: &[u8]) -> Result<Self, SnapshotError> {
        check_header(bytes)?;
        options()
            .deserialize(bytes)
            .map_err(|error| SnapshotError::Damaged(error.to_string()))
    }
}

// The two header fields come first in both `UnitSnapshot` and `CacheEntry`, so a newer layout
// is reported as such and not as damage.
fn check_header(bytes: &[u8]) -> Result<(), SnapshotError> {
    let (format, enc_version): (u32, u8) = options()
        .allow_trailing_bytes()
        .deserialize(bytes)
        .map_err(|error| SnapshotError::Damaged(error.to_string()))?;
    if format != SNAPSHOT_FORMAT {
        return Err(SnapshotError::Format { found: format });
    }
    if enc_version != ENC_VERSION {
        return Err(SnapshotError::Encoding { found: enc_version });
    }
    Ok(())
}

// Fixed-width integers and a size bound, so the header is at a known place and the bytes do
// not depend on bincode's defaults.
fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(DECODE_LIMIT)
}

/// A stored unit output and the questions whose answers it depends on.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Variant {
    pub probes: Vec<ProbeRecord>,
    pub snapshot: UnitSnapshot,
}

/// What one cache key holds: the outputs one file had under different answers from the rest of
/// the project (a file checked in two passes, or in two projects), newest last. Only a
/// variant whose every probe still holds is used, so an entry never answers for a project it
/// was not made for.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CacheEntry {
    format: u32,
    enc_version: u8,
    variants: Vec<Variant>,
}

/// How many outputs one entry keeps.
const MAX_VARIANTS: usize = 6;

impl CacheEntry {
    pub fn new() -> Self {
        Self {
            format: SNAPSHOT_FORMAT,
            enc_version: ENC_VERSION,
            variants: Vec::new(),
        }
    }

    /// The newest stored output whose probes all hold against `source`, with the declarations
    /// of other files that check read (sorted, without duplicates), which is what a fresh
    /// check would have reported. `source` should be a view that records nothing: a variant
    /// that fails part-way has already asked some of its questions.
    pub fn lookup(&self, source: &dyn ImportSource) -> Option<(UnitOutput, Vec<DeclKey>)> {
        let variant = self
            .variants
            .iter()
            .rev()
            .find(|variant| all_hold(&variant.probes, source))?;
        Some((
            variant.snapshot.clone().into_output(),
            crate::probe_log::declarations_read(&variant.probes),
        ))
    }

    /// Adds an output, replacing one made under the same answers, and drops the oldest past
    /// the limit.
    pub fn push(&mut self, variant: Variant) {
        self.variants.retain(|kept| kept.probes != variant.probes);
        self.variants.push(variant);
        if self.variants.len() > MAX_VARIANTS {
            self.variants.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.variants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.variants.is_empty()
    }

    pub fn encode(&self) -> Result<Vec<u8>, SnapshotError> {
        options()
            .serialize(self)
            .map_err(|error| SnapshotError::Damaged(error.to_string()))
    }

    /// Reads an entry, refusing one written under another format or key encoding.
    pub fn decode(bytes: &[u8]) -> Result<Self, SnapshotError> {
        check_header(bytes)?;
        options()
            .deserialize(bytes)
            .map_err(|error| SnapshotError::Damaged(error.to_string()))
    }
}
