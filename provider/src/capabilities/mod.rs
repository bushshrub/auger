//! Optional provider capabilities beyond `complete`/`stream`.
//!
//! Each capability is its own trait with [`LlmProvider`](crate::LlmProvider)
//! as a supertrait: only providers can claim capabilities, but whether one
//! does is a compile-time fact of its concrete type.
//!
//! There is no runtime
//! discovery on `LlmProvider` itself (for now) - resolve capabilities where the
//! concrete provider type is in hand (e.g. at provider registration) and
//! pass the resulting handles to whatever needs them.

mod catalog;

pub use catalog::ModelCatalog;
pub use catalog::ModelId;
pub use catalog::ModelInfo;
pub use catalog::resolve_model;
