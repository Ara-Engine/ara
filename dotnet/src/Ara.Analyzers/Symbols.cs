using System.Linq;
using Microsoft.CodeAnalysis;

namespace Ara.Analyzers;

/// <summary>Symbol helpers shared by the analyzers.</summary>
public static class Symbols
{
    /// <summary>
    /// True when the compilation is a gameplay module: it references Ara.Runtime and is
    /// not part of the engine itself. The analyzers stay silent elsewhere.
    /// </summary>
    public static bool IsGameplayModule(Compilation c) =>
        c.AssemblyName is not ("Ara.Runtime" or "Ara.Host" or "Ara.Agent")
        && c.ReferencedAssemblyNames.Any(a => a.Name == "Ara.Runtime");

    /// <summary>True for <c>Ara.{name}</c>.</summary>
    public static bool IsAra(ITypeSymbol? t, string name) =>
        t is INamedTypeSymbol { ContainingNamespace: { Name: "Ara", ContainingNamespace.IsGlobalNamespace: true } } n && n.Name == name;

    /// <summary>True if <paramref name="t"/> derives from <c>Ara.Behaviour</c>.</summary>
    public static bool IsBehaviour(INamedTypeSymbol? t)
    {
        for (var b = t?.BaseType; b is not null; b = b.BaseType)
            if (IsAra(b, "Behaviour")) return true;
        return false;
    }

    /// <summary>True if the symbol carries an attribute of class <c>Ara.{name}</c>.</summary>
    public static bool HasAra(ISymbol s, string name) =>
        s.GetAttributes().Any(a => IsAra(a.AttributeClass, name));

    /// <summary>True if the symbol or a containing type, or the assembly, is <c>[Simulation]</c>.</summary>
    public static bool InSimulation(ISymbol s)
    {
        if (HasAra(s.ContainingAssembly, "SimulationAttribute")) return true;
        for (var t = s as INamedTypeSymbol ?? s.ContainingType; t is not null; t = t.ContainingType)
            if (HasAra(t, "SimulationAttribute")) return true;
        return false;
    }

    /// <summary>
    /// True for methods that run on the tick path: <c>[Query]</c> systems, behaviour
    /// <c>OnTick</c>/<c>OnFrame</c> overrides, and command <c>Execute</c> methods.
    /// </summary>
    public static bool IsTickEntry(IMethodSymbol m)
    {
        if (HasAra(m, "QueryAttribute")) return true;
        if (m.IsOverride && m.Name is "OnTick" or "OnFrame" && IsBehaviour(m.ContainingType)) return true;
        return m.Name == "Execute" && m.IsStatic && HasAra(m.ContainingType, "CommandAttribute");
    }
}
