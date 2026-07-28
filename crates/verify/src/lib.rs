//! Rust harness for shellac's PDF incremental-save verification.
//!
//! Orchestrates external tools (qpdf, pdftotext, the PDFKit helpers, and
//! the save-engine CLI under test) over a sample corpus and emits a
//! markdown verification matrix.

pub mod anchor;
pub mod checks;
pub mod cli;
pub mod consts;
pub mod encrypted;
pub mod engine;
pub mod exceptions;
pub mod geom;
pub mod ids;
pub mod matrix;
pub mod proc;
pub mod report;
pub mod sample;
pub mod sanitize;
pub mod scenario;
pub mod state;
pub mod swift;
pub mod util;
