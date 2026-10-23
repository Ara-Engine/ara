using System.Diagnostics.CodeAnalysis;

namespace Ara.Hosting;

/// <summary>
/// Emitted by the generator on every gameplay module assembly: its name and the
/// generated class whose static <c>Register(ModuleBuilder)</c> declares the module. The
/// dev loader finds it on each loaded generation; NativeAOT entry points link it directly.
/// </summary>
[AttributeUsage(AttributeTargets.Assembly)]
public sealed class AraModuleAttribute(string name, [DynamicallyAccessedMembers(DynamicallyAccessedMemberTypes.PublicMethods)] Type entry) : Attribute
{
    /// <summary>The module name (<c>ara.module.toml</c>).</summary>
    public string Name { get; } = name;

    /// <summary>The generated registration class.</summary>
    [DynamicallyAccessedMembers(DynamicallyAccessedMemberTypes.PublicMethods)]
    public Type Entry { get; } = entry;
}
