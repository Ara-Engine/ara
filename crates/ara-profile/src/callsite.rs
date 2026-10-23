/// Immutable metadata shared by every invocation of one instrumentation site.
/// Construct with `callsite!`; use `scope!` for a lexical scope.
pub struct Callsite {
    name: &'static str,
    #[cfg(feature = "tracy")]
    location: fn() -> &'static tracy_client::SpanLocation,
}
impl Callsite {
    /// Creates native static metadata; `callsite!` supplies this automatically.
    #[cfg(not(feature = "tracy"))]
    pub const fn new(name: &'static str) -> Self {
        Self { name }
    }
    /// Creates metadata with a once-initialized, static Tracy source location.
    #[cfg(feature = "tracy")]
    pub const fn new(
        name: &'static str,
        location: fn() -> &'static tracy_client::SpanLocation,
    ) -> Self {
        Self { name, location }
    }
    /// Stable display name, resolved outside the native producer hot loop.
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// Backend-selected scope guard. Tracy replaces native recording for these sites.
#[cfg(feature = "tracy")]
pub type ScopeGuard = Option<tracy_client::Span>;
/// Backend-selected scope guard for native frame inspection.
#[cfg(not(feature = "tracy"))]
pub type ScopeGuard = crate::SpanGuard;

/// Starts a site with an explicit lifetime. `scope!` is preferred for hot loops.
/// Disabled macros emit no code; a stopped capture takes only one relaxed load.
#[inline(always)]
pub fn scope_at(site: &'static Callsite) -> ScopeGuard {
    #[cfg(feature = "tracy")]
    {
        if crate::instrumentation_enabled() {
            tracy_client::Client::running().map(|client| client.span((site.location)(), 0))
        } else {
            None
        }
    }
    #[cfg(not(feature = "tracy"))]
    {
        if cfg!(feature = "instrumentation") && crate::instrumentation_enabled() {
            crate::SpanGuard::at(site)
        } else {
            crate::SpanGuard::inactive()
        }
    }
}

/// Selected macro backend: `disabled`, `native`, or `tracy`.
pub const fn scope_backend() -> &'static str {
    if cfg!(feature = "tracy") {
        "tracy"
    } else if cfg!(feature = "instrumentation") {
        "native"
    } else {
        "disabled"
    }
}

/// Defines one immutable callsite. Source metadata is initialized once by Tracy.
#[cfg(feature = "tracy")]
#[macro_export]
macro_rules! callsite {
    ($name:literal) => {{
        static SITE: $crate::Callsite =
            $crate::Callsite::new($name, || $crate::__tracy::span_location!($name));
        &SITE
    }};
}
/// Defines one immutable native callsite, without runtime name construction.
#[cfg(not(feature = "tracy"))]
#[macro_export]
macro_rules! callsite {
    ($name:literal) => {{
        static SITE: $crate::Callsite = $crate::Callsite::new($name);
        &SITE
    }};
}

/// Instruments the remainder of the enclosing block. Shipping omits the
/// `instrumentation` feature, making this macro expand to nothing.
#[cfg(feature = "instrumentation")]
#[macro_export]
macro_rules! scope {
    ($name:literal) => {
        let _ara_scope_guard = $crate::scope_at($crate::callsite!($name));
    };
}
/// Compiled-out instrumentation: no metadata, argument evaluation, TLS or clock.
#[cfg(not(feature = "instrumentation"))]
#[macro_export]
macro_rules! scope {
    ($name:literal) => {};
}
