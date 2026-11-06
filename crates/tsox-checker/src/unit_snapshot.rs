//! A byte form for what a unit hands back, so the result of checking a bundled library file
//! can be stored and read again instead of being computed on every run.
//!
//! A snapshot holds the whole of a [`UnitOutput`] except its diagnostics: a library file is
//! never reported on, and a snapshot of one that had diagnostics is refused at capture, so a
//! reader never gets a quieter unit than the one that was checked. What it holds is arena-free
//! data (LLD 2.1 rule 3), so a decoded snapshot is usable in any worker.
//!
//! The header carries the format number of this module and `ENC_VERSION` of the key
//! encoding. A reader that sees either differ gets an error and recomputes; it never reads
//! bytes written under another encoding.

use bincode::Options;
use thiserror::Error;
use tsox_base::keys::{DeclKey, ENC_VERSION};
use tsox_types::portable::PortableDecl;

use crate::bridge::IncompleteReason;
use crate::unit::{AmbientExports, ReturnRead, UnitOutput};

/// Bump this when the fields below, or the meaning of one, change.
pub const SNAPSHOT_FORMAT: u32 = 1;

/// Bytes a snapshot may take when decoded. A library file's output is a few hundred kilobytes;
/// the bound only stops a damaged length prefix from asking for a huge allocation.
const DECODE_LIMIT: u64 = 256 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("the unit has diagnostics, so it is not a library snapshot")]
    HasDiagnostics,
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
    /// What `output` holds, or why it cannot be a snapshot.
    pub fn capture(output: &UnitOutput) -> Result<Self, SnapshotError> {
        if !output.diagnostics.is_empty() {
            return Err(SnapshotError::HasDiagnostics);
        }
        Ok(Self {
            format: SNAPSHOT_FORMAT,
            enc_version: ENC_VERSION,
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
        })
    }

    /// The unit output this snapshot was taken from, with no diagnostics.
    pub fn into_output(self) -> UnitOutput {
        UnitOutput {
            diagnostics: Vec::new(),
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
        options()
            .deserialize(bytes)
            .map_err(|error| SnapshotError::Damaged(error.to_string()))
    }
}

// Fixed-width integers and a size bound, so the header is at a known place and the bytes do
// not depend on bincode's defaults.
fn options() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(DECODE_LIMIT)
}
