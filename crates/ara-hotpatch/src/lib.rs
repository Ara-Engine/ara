//! Source compatibility checks and opt-in Subsecond runtime for local development.
//! The default library is tooling only; shipping hosts never enable `runtime`.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod contract;
pub use contract::Contract;

#[cfg(all(feature = "runtime", not(debug_assertions)))]
compile_error!("Ara Rust hotpatch is development-only; rebuild without the hotpatch feature");

#[cfg(feature = "runtime")]
mod runtime;
#[cfg(feature = "runtime")]
pub use runtime::{DETERMINISM, Hotpatch, Lease, Status, args_os, generation};

#[cfg(feature = "runtime")]
#[doc(hidden)]
pub use subsecond;

/// Call a synchronous, layout-stable function through Subsecond while pinning its generation.
/// Never call `frame_boundary` inside this scope. Its arguments must not retain code across frames.
#[cfg(feature = "runtime")]
#[macro_export]
macro_rules! call {
    ($function:expr $(, $arg:expr)* $(,)?) => {{
        let _lease = $crate::Lease::enter();
        $crate::subsecond::HotFn::current($function).call(($($arg,)*))
    }};
}
