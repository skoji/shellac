//! Individual check implementations. Pure evaluation logic lives here;
//! external-process plumbing stays thin and separable so the judgment
//! semantics are unit-testable.

pub mod audit;
pub mod bbox;
pub mod enc_qpdf;
pub mod pdfkit;
pub mod prefix;
pub mod producer;
pub mod qpdf;
pub mod text;
