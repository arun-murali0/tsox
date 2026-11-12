//! A language server for tsox, built on tower.
//!
//! Phase 1 is the layer under every request: positions and edits (`text`), the
//! conversion from tsox diagnostics to LSP diagnostics (`convert`), and the mapping
//! between document URIs and the paths the project layer keys files by (`paths`).
//!
//! Phase 2 is `worker`: one thread that owns the incremental project, takes the editor's
//! changes as commands, cancels a check that a newer change has overtaken, and reports the
//! diagnostics that changed.
//!
//! Phase 3 is `server`: the tower service that speaks LSP (open, change, close, watched
//! files) and publishes what the worker reports. The `tsox-lsp` binary serves it over stdio.

pub mod analysis;
pub mod completion;
pub mod convert;
pub mod hover;
pub mod navigate;
pub mod paths;
pub mod server;
pub mod signature;
pub mod structure;
pub mod symbols;
pub mod text;
pub mod worker;
