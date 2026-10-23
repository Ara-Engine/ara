using System;
using System.Collections;
using System.Collections.Generic;
using System.Collections.Immutable;
using System.Linq;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Text;

namespace Ara.Generators;

/// <summary>An immutable array with value equality, so incremental pipeline models cache.</summary>
public readonly struct EquatableArray<T>(ImmutableArray<T> items) : IEquatable<EquatableArray<T>>, IEnumerable<T>
    where T : IEquatable<T>
{
    readonly ImmutableArray<T> _items = items;

    /// <summary>The items (empty when default).</summary>
    public ImmutableArray<T> Items => _items.IsDefault ? ImmutableArray<T>.Empty : _items;

    /// <summary>Number of items.</summary>
    public int Length => Items.Length;

    /// <summary>Item at <paramref name="i"/>.</summary>
    public T this[int i] => Items[i];

    /// <inheritdoc/>
    public bool Equals(EquatableArray<T> other) => Items.SequenceEqual(other.Items);

    /// <inheritdoc/>
    public override bool Equals(object? obj) => obj is EquatableArray<T> o && Equals(o);

    /// <inheritdoc/>
    public override int GetHashCode()
    {
        var h = 17;
        foreach (var i in Items) h = unchecked(h * 31 + i.GetHashCode());
        return h;
    }

    /// <inheritdoc/>
    public IEnumerator<T> GetEnumerator() => ((IEnumerable<T>)Items).GetEnumerator();

    IEnumerator IEnumerable.GetEnumerator() => GetEnumerator();
}

/// <summary>Helpers to build <see cref="EquatableArray{T}"/>s.</summary>
public static class EquatableArray
{
    /// <summary>Wraps a sequence.</summary>
    public static EquatableArray<T> ToEquatable<T>(this IEnumerable<T> items) where T : IEquatable<T> => new([.. items]);
}

/// <summary>A diagnostic to report, with an equatable location.</summary>
public sealed record DiagInfo(DiagnosticDescriptor Descriptor, string Message, string? File, TextSpan Span, LinePositionSpan Lines)
{
    /// <summary>Captures a diagnostic at a symbol's first declaration.</summary>
    public static DiagInfo At(DiagnosticDescriptor d, ISymbol? symbol, string message)
    {
        var loc = symbol?.Locations.FirstOrDefault(l => l.IsInSource);
        return new(d, message, loc?.SourceTree?.FilePath, loc?.SourceSpan ?? default, loc?.GetLineSpan().Span ?? default);
    }

    /// <summary>Back to a Roslyn diagnostic.</summary>
    public Diagnostic ToDiagnostic() =>
        Diagnostic.Create(Descriptor, File is null ? Location.None : Location.Create(File, Span, Lines), Message);
}

/// <summary>Where a partial type lives: namespace, enclosing types and its own header.</summary>
public sealed record Shell(string? Namespace, EquatableArray<string> Containers, string Header, string FullName);

/// <summary>One <c>types.json</c> entry: schema name and its JSON definition.</summary>
public sealed record TypeDef(string Name, string Json);

/// <summary>A <c>[Component]</c>, <c>[Resource]</c> or <c>[Cue]</c> struct.</summary>
public sealed record DataModel(Shell Shell, string Role, string SchemaName, EquatableArray<string> Fields,
    EquatableArray<TypeDef> Defs, EquatableArray<DiagInfo> Diags);

/// <summary>One parameter of a system's <c>Run</c> method.</summary>
public sealed record SystemParam(string Kind, string CsType, string RefKind, string SchemaName);

/// <summary>A <c>[Query]</c> system.</summary>
public sealed record SystemModel(Shell Shell, string Method, string Name, int Order, EquatableArray<SystemParam> Params,
    EquatableArray<string> With, EquatableArray<string> Without, EquatableArray<DiagInfo> Diags);

/// <summary>One <c>[Property]</c> of a behaviour.</summary>
public sealed record PropModel(string Modifiers, string CsType, string Name, bool HasGet, string? SetKeyword, string Doc);

/// <summary>A <c>Behaviour</c> subclass.</summary>
public sealed record BehaviourModel(Shell Shell, string SchemaName, EquatableArray<PropModel> Props,
    EquatableArray<TypeDef> Defs, EquatableArray<DiagInfo> Diags);

/// <summary>One parameter of a command's <c>Execute</c> method.</summary>
public sealed record CommandParam(string Kind, string CsType, string Name, string RefKind, string SchemaName);

/// <summary>A <c>[Command]</c> class.</summary>
public sealed record CommandModel(Shell Shell, string Name, string Summary, string ParamsType, EquatableArray<CommandParam> Params,
    string? Expose, EquatableArray<string> Examples, EquatableArray<TypeDef> Defs, EquatableArray<DiagInfo> Diags);

/// <summary>A module linked from a referenced assembly (for the NativeAOT entry point).</summary>
public sealed record LinkedModule(string Name, string Entry);
