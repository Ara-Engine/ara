using Microsoft.CodeAnalysis;

namespace Ara.Generators;

/// <summary>Diagnostics of the Ara source generator (ARA0700-0799).</summary>
public static class Descriptors
{
    const string Category = "Ara.Generator";

    static DiagnosticDescriptor Error(string id, string title) =>
        new(id, title, "{0}", Category, DiagnosticSeverity.Error, isEnabledByDefault: true);

    /// <summary>A type the generator extends must be <c>partial</c> (and so must its enclosing types).</summary>
    public static readonly DiagnosticDescriptor NotPartial = Error("ARA0701", "Type must be partial");

    /// <summary>A system parameter kind the generator cannot bind.</summary>
    public static readonly DiagnosticDescriptor BadSystemParam = Error("ARA0702", "Unsupported system parameter");

    /// <summary>Reflected types and fields need doc comments: they become tooltips, docs and agent tool descriptions.</summary>
    public static readonly DiagnosticDescriptor MissingDoc = Error("ARA0703", "Reflected type or field needs a doc comment");

    /// <summary>A component field whose type has no blittable schema type.</summary>
    public static readonly DiagnosticDescriptor NotBlittable = Error("ARA0704", "Field type is not blittable");

    /// <summary>A <c>[Command]</c> class without a valid <c>Execute</c> method.</summary>
    public static readonly DiagnosticDescriptor BadCommand = Error("ARA0705", "Invalid command shape");

    /// <summary>A behaviour the runtime cannot construct.</summary>
    public static readonly DiagnosticDescriptor BadBehaviour = Error("ARA0706", "Invalid behaviour");

    /// <summary>A <c>[Query]</c> method that is not static, returns a value or is generic.</summary>
    public static readonly DiagnosticDescriptor BadSystem = Error("ARA0707", "Invalid system method");
}
