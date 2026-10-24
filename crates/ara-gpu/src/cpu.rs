//! Optional literal CPU zones shared by renderer crates without a dependency cycle.

/// Records a literal CPU zone if the executable started a Tracy client.
/// Default builds erase the zone; library tests do not have to start a client.
#[cfg(feature = "tracy")]
#[macro_export]
macro_rules! cpu_scope {
    ($name:literal) => {
        let _ara_cpu_scope = $crate::__tracy::Client::running()
            .map(|client| client.span($crate::__tracy::span_location!($name), 0));
    };
}

/// Records a literal CPU zone if the executable started a Tracy client.
/// Default builds erase the zone; library tests do not have to start a client.
#[cfg(not(feature = "tracy"))]
#[macro_export]
macro_rules! cpu_scope {
    ($name:literal) => {};
}
