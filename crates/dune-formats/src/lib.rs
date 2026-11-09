//! Readers for the data formats used by Cryo's *Dune* (CD version, 1993).
//!
//! Nothing here ships game data: everything is read from the user's own `DUNE.DAT`.

pub mod dat;
pub mod dialogue;
pub mod face;
pub mod globe;
pub mod herad;
pub mod hnm;
pub mod hsq;
pub mod palette;
pub mod planet;
pub mod sal;
pub mod save;
pub mod sprite;
pub mod text;
pub mod voc;

pub use dat::{DatArchive, DatEntry};
