using System.Collections.Generic;
using System.Collections.Immutable;
using System.Globalization;
using System.Linq;
using Microsoft.CodeAnalysis;

namespace Ara.Generators;

/// <summary>A reflected member: a component field, behaviour property or command parameter.</summary>
public readonly record struct Member(string Name, ITypeSymbol Type, string Doc, ImmutableArray<AttributeData> Attributes,
    ISymbol Symbol, bool Required = false, string? Default = null);

/// <summary>
/// Maps C# declarations to Schema DB type definitions (the <c>types.json</c> format the
/// host registers): field types, units, ranges, defaults, replication, aliases, docs.
/// </summary>
public static class Schema
{
    /// <summary>The assembly whose types the engine already describes (no defs are emitted for them).</summary>
    public const string RuntimeAssembly = "Ara.Runtime";

    /// <summary><c>Hp</c> → <c>hp</c>.</summary>
    public static string Camel(string s) => s.Length == 0 ? s : char.ToLowerInvariant(s[0]) + s.Substring(1);

    /// <summary><c>actor</c> → <c>Actor</c>.</summary>
    public static string Pascal(string s) => s.Length == 0 ? s : char.ToUpperInvariant(s[0]) + s.Substring(1);

    /// <summary>True for <c>[A-Z][A-Za-z0-9]*</c>.</summary>
    public static bool IsTypeName(string s) => s.Length > 0 && s[0] >= 'A' && s[0] <= 'Z' && s.All(char.IsLetterOrDigit);

    /// <summary>The first attribute of class <c>Ara.{name}</c> (generic ones match by their definition's name).</summary>
    public static AttributeData? Attr(ISymbol s, string name) =>
        s.GetAttributes().FirstOrDefault(a => IsAra(a.AttributeClass, name));

    /// <summary>Every attribute of class <c>Ara.{name}</c>.</summary>
    public static IEnumerable<AttributeData> Attrs(ISymbol s, string name) =>
        s.GetAttributes().Where(a => IsAra(a.AttributeClass, name));

    static bool IsAra(INamedTypeSymbol? c, string name) =>
        c is not null && c.Name == name && c.ContainingNamespace is { Name: "Ara", ContainingNamespace.IsGlobalNamespace: true };

    /// <summary>True for <c>Ara.{name}</c>.</summary>
    public static bool IsAraType(ITypeSymbol? t, string name) => t is INamedTypeSymbol n && IsAra(n, name);

    static string? StringArg(AttributeData a) =>
        a.ConstructorArguments.Length > 0 && a.ConstructorArguments[0].Value is string s ? s : null;

    /// <summary>True if <paramref name="t"/> is a component (<c>[Component]</c> or engine-generated); gives its schema name.</summary>
    public static bool IsComponent(ITypeSymbol t, out string name)
    {
        var a = Attr(t, "ComponentAttribute") ?? Attr(t, "EngineComponentAttribute");
        name = a is null ? "" : StringArg(a) ?? t.Name;
        return a is not null;
    }

    /// <summary>True if <paramref name="t"/> is a <c>[Resource]</c>; gives its schema name.</summary>
    public static bool IsResource(ITypeSymbol t, out string name)
    {
        var a = Attr(t, "ResourceAttribute");
        name = a is null ? "" : StringArg(a) ?? t.Name;
        return a is not null;
    }

    /// <summary>The schema name a data attribute gives a type.</summary>
    public static string DataName(INamedTypeSymbol t, AttributeData a) => StringArg(a) ?? t.Name;

    /// <summary>
    /// The schema type of a C# field type (<c>f32</c>, <c>vec3</c>, <c>Mode</c>), adding
    /// definitions for enums and value structs to <paramref name="defs"/>. Null if it is not blittable.
    /// </summary>
    public static string? FieldType(ITypeSymbol t, List<TypeDef> defs, List<DiagInfo> diags, ISymbol at, int depth = 0)
    {
        switch (t.SpecialType)
        {
            case SpecialType.System_Boolean: return "bool";
            case SpecialType.System_Byte: return "u8";
            case SpecialType.System_UInt16: return "u16";
            case SpecialType.System_Int32: return "i32";
            case SpecialType.System_UInt32: return "u32";
            case SpecialType.System_Int64: return "i64";
            case SpecialType.System_UInt64: return "u64";
            case SpecialType.System_Single: return "f32";
            case SpecialType.System_Double: return "f64";
        }
        if (t is INamedTypeSymbol { ContainingNamespace: { Name: "Numerics", ContainingNamespace.Name: "System" } } num)
        {
            switch (num.Name)
            {
                case "Vector2": return "vec2";
                case "Vector3": return "vec3";
                case "Vector4": return "vec4";
                case "Quaternion": return "quat";
            }
        }
        if (IsAraType(t, "Color")) return "color";
        if (IsAraType(t, "Entity")) return "entity";
        if (t is INamedTypeSymbol { TypeKind: TypeKind.Enum } e) return EnumType(e, defs, diags, at);
        if (t is INamedTypeSymbol { TypeKind: TypeKind.Struct, IsUnmanagedType: true } s && depth < 8)
        {
            if (t.ContainingAssembly?.Name == RuntimeAssembly) return s.Name; // engine value type, described in Rust
            if (defs.All(d => d.Name != s.Name))
            {
                defs.Add(new TypeDef(s.Name, "")); // placeholder: stops recursion through this type
                var json = StructJson(s, "struct", Fields(s), defs, diags, depth + 1);
                defs.RemoveAll(d => d.Name == s.Name);
                defs.Add(new TypeDef(s.Name, json));
            }
            return s.Name;
        }
        diags.Add(DiagInfo.At(Descriptors.NotBlittable, at,
            $"`{at.Name}` has type `{t.ToDisplayString()}`, which has no blittable schema type; use numbers, bool, vectors, Quaternion, Color, Entity, enums or structs of those"));
        return null;
    }

    static string? EnumType(INamedTypeSymbol e, List<TypeDef> defs, List<DiagInfo> diags, ISymbol at)
    {
        if (e.ContainingAssembly?.Name == RuntimeAssembly) return e.Name;
        var members = e.GetMembers().OfType<IFieldSymbol>().Where(f => f.HasConstantValue).ToList();
        var u = e.EnumUnderlyingType?.SpecialType;
        var contiguous = members.Select((m, i) => System.Convert.ToInt64(m.ConstantValue, CultureInfo.InvariantCulture) == i).All(x => x);
        if (u is not (SpecialType.System_Int32 or SpecialType.System_UInt32) || !contiguous)
        {
            diags.Add(DiagInfo.At(Descriptors.NotBlittable, at,
                $"enum `{e.Name}` must have an `int` or `uint` base and values 0, 1, 2, ... in order (the schema stores the variant index)"));
            return null;
        }
        if (defs.Any(d => d.Name == e.Name)) return e.Name;
        var j = new Json().Open().Key("kind").String("enum").Key("doc").String(RequireDoc(e, diags)).Key("variants").Open();
        foreach (var m in members)
            j.Key(Camel(m.Name)).Open().Key("doc").String(RequireDoc(m, diags)).Close();
        defs.Add(new TypeDef(e.Name, j.Close().Close().ToString()));
        return e.Name;
    }

    /// <summary>The instance fields of a struct, in declaration order.</summary>
    public static List<Member> Fields(INamedTypeSymbol s) =>
        s.GetMembers().OfType<IFieldSymbol>().Where(f => !f.IsStatic && !f.IsConst)
            .Select(f => new Member(f.Name, f.Type, Docs.Summary(f), f.GetAttributes(), f)).ToList();

    /// <summary>A doc comment, or an <c>ARA0703</c> diagnostic.</summary>
    public static string RequireDoc(ISymbol s, List<DiagInfo> diags, string? doc = null)
    {
        doc ??= Docs.Summary(s);
        if (string.IsNullOrWhiteSpace(doc))
            diags.Add(DiagInfo.At(Descriptors.MissingDoc, s,
                $"`{s.Name}` is reflected into the Schema DB and needs a `/// <summary>` doc comment (it becomes the tooltip and the agent tool description)"));
        return doc;
    }

    /// <summary>A struct definition (<c>component</c>, <c>resource</c>, <c>struct</c> or <c>params</c>) as types.json JSON.</summary>
    public static string StructJson(ISymbol type, string kind, IEnumerable<Member> members, List<TypeDef> defs,
        List<DiagInfo> diags, int depth = 0, string? doc = null)
    {
        var j = new Json().Open().Key("kind").String(kind).Key("doc").String(RequireDoc(type, diags, doc)).Key("fields").Open();
        foreach (var m in members)
        {
            var ty = FieldType(m.Type, defs, diags, m.Symbol, depth);
            if (ty is null) continue;
            var tooltip = m.Attributes.FirstOrDefault(a => IsAra(a.AttributeClass, "TooltipAttribute"));
            var fieldDoc = m.Doc.Length > 0 ? m.Doc : tooltip is null ? "" : StringArg(tooltip) ?? "";
            j.Key(Camel(m.Name)).Open().Key("type").String(ty).Key("doc").String(RequireDoc(m.Symbol, diags, fieldDoc));
            if (m.Required) j.Key("required").Bool(true);
            if (m.Default is { } d) j.Key("default").Raw(d);
            WriteFieldAttributes(j, m.Attributes);
            j.Close();
        }
        return j.Close().Close().ToString();
    }

    static void WriteFieldAttributes(Json j, ImmutableArray<AttributeData> attrs)
    {
        double? min = null, max = null;
        var aliases = new List<string>();
        foreach (var a in attrs)
        {
            var c = a.AttributeClass;
            var args = a.ConstructorArguments;
            if (IsAra(c, "UnitAttribute") && StringArg(a) is { } unit) j.Key("unit").String(unit);
            else if (IsAra(c, "RangeAttribute") && args.Length == 2) { min = Num(args[0]); max = Num(args[1]); }
            else if (IsAra(c, "MinAttribute") && args.Length == 1) min = Num(args[0]);
            else if (IsAra(c, "MaxAttribute") && args.Length == 1) max = Num(args[0]);
            else if (IsAra(c, "DefaultAttribute") && args.Length == 1)
            {
                var json = a.NamedArguments.FirstOrDefault(p => p.Key == "Json").Value.Value as string;
                j.Key("default").Raw(json ?? DefaultJson(args[0]));
            }
            else if (IsAra(c, "ReplicateAttribute")) j.Key("replicate").Raw(ReplicateJson(a));
            else if (IsAra(c, "FormerlySerializedAsAttribute") && StringArg(a) is { } old) aliases.Add(Camel(old));
            else if (IsAra(c, "HeaderAttribute") && StringArg(a) is { } group) j.Key("editor").Open().Key("group").String(group).Close();
            else if (c?.ToDisplayString() == "System.ObsoleteAttribute") j.Key("deprecated").String(StringArg(a) ?? "deprecated");
        }
        if (min is not null || max is not null)
        {
            j.Key("range").OpenArray();
            if (min is { } lo) j.Number(lo); else j.Null();
            if (max is { } hi) j.Number(hi); else j.Null();
            j.CloseArray();
        }
        if (aliases.Count > 0)
        {
            j.Key("aliases").OpenArray();
            foreach (var al in aliases) j.String(al);
            j.CloseArray();
        }
    }

    static double? Num(TypedConstant c) => c.Value is null ? null : System.Convert.ToDouble(c.Value, CultureInfo.InvariantCulture);

    /// <summary>A <c>[Default(...)]</c> argument (params object[]) as JSON: one value, or an array for vectors.</summary>
    static string DefaultJson(TypedConstant arg)
    {
        var values = arg.Kind == TypedConstantKind.Array ? arg.Values : ImmutableArray.Create(arg);
        static string One(TypedConstant v) => v.Value switch
        {
            bool b => b ? "true" : "false",
            string s => new Json().String(s).ToString(),
            null => "null",
            var n => System.Convert.ToDouble(n, CultureInfo.InvariantCulture).ToString("R", CultureInfo.InvariantCulture),
        };
        return values.Length == 1 ? One(values[0]) : "[" + string.Join(",", values.Select(One)) + "]";
    }

    static readonly string[] Audiences = ["none", "relevant", "owner"];
    static readonly string[] Interpolations = ["none", "linear", "slerp"];

    static string ReplicateJson(AttributeData a)
    {
        var audience = a.ConstructorArguments.Length > 0 && a.ConstructorArguments[0].Value is int i ? i : 1;
        var j = new Json().Open().Key("audience").String(Audiences[System.Math.Min(System.Math.Max(audience, 0), 2)]);
        foreach (var kv in a.NamedArguments)
        {
            switch (kv.Key)
            {
                case "Quantize" when kv.Value.Value is double q && q > 0: j.Key("quantize").Number(q); break;
                case "Interpolate" when kv.Value.Value is int m && m > 0: j.Key("interpolate").String(Interpolations[System.Math.Min(m, 2)]); break;
                case "Rollback" when kv.Value.Value is true: j.Key("rollback").Bool(true); break;
            }
        }
        return j.Close().ToString();
    }

    /// <summary>An <c>[Expose]</c> attribute as JSON, or null.</summary>
    public static string? ExposeJson(ISymbol t)
    {
        if (Attr(t, "ExposeAttribute") is not { } a || a.ConstructorArguments.Length < 2) return null;
        string[] on = ["authority", "predicted"], caller = ["any", "owner", "authority"];
        var j = new Json().Open()
            .Key("executeOn").String(on[System.Math.Min((int)(a.ConstructorArguments[0].Value ?? 0), 1)])
            .Key("caller").String(caller[System.Math.Min((int)(a.ConstructorArguments[1].Value ?? 0), 2)]);
        foreach (var kv in a.NamedArguments)
            if (kv.Key == "RatePerSecond" && kv.Value.Value is double r && r > 0) j.Key("ratePerSecond").Number(r);
        return j.Close().ToString();
    }
}
