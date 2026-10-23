// Polyfills so netstandard2.0 compiles records and init-only setters.
namespace System.Runtime.CompilerServices;

/// <summary>Marker type the compiler needs for <c>init</c> accessors on netstandard2.0.</summary>
internal static class IsExternalInit;
