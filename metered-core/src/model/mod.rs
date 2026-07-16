//! The metric-tree model: the core traits, metadata, schema/values pair, and
//! tree shaping.

mod metric_impls;

// Hidden at the definition, not only at the crate-root re-export, so facades
// glob-re-exporting this crate keep it out of their docs too.
#[doc(hidden)]
pub mod handle;
pub mod meta;
pub mod metric_tree;
pub mod schema;
pub mod shape;
pub mod values;
