//! Nonblocking editor controls over the existing Play owner links. Runtime reads
//! and patches finish through Host jobs; Keep returns to the caller's typed transaction context.
use crate::*;
mod jobs;
mod view_store;
pub(crate) use jobs::Pending;
pub use view_store::{ViewPose, ViewStore, property_ops};
