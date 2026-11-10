using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class AnalyzerCoverageTests
{
    [Fact]
    public void A_partial_property_body_rechecks_the_definition_when_it_introduces_storage()
    {
        using var modules = new Modules();
        Directory.CreateDirectory(Path.Combine(modules.Root, "Game"));
        var body = modules.File("Game", "Property.cs");
        const string original = "public partial class Settings { public static partial int Value { get => 0; set { } } }";
        File.WriteAllText(body, original);
        modules.Add(0, "Game", "public partial class Settings { public static partial int Value { get; set; } }");
        modules.Idle();
        Assert.Equal(MessageKind.Reload, modules.Sink.Next().Kind);
        var good = modules.Server.Dependency("Game");
        File.WriteAllText(body, original.Replace("=> 0", "=> field", StringComparison.Ordinal));
        modules.Server.Change(0, [body], AbiConst.OriginHuman, 0);
        modules.Idle();
        var rejected = modules.Sink.Next();
        Assert.Equal(MessageKind.Diagnostics, rejected.Kind);
        Assert.Contains("ARA0100", rejected.Diagnostics().Json);
        Assert.Same(good, modules.Server.Dependency("Game"));
        File.WriteAllText(body, original);
        modules.Server.Change(0, [body], AbiConst.OriginHuman, 0);
        modules.Idle();
        Assert.Same(good, modules.Server.Dependency("Game"));
        Assert.True(modules.Sink.Empty);
    }

    [Fact]
    public void A_simulation_attribute_rechecks_unchanged_partial_method_bodies()
    {
        using var modules = new Modules();
        Directory.CreateDirectory(Path.Combine(modules.Root, "Game"));
        var body = modules.File("Game", "Clock.cs");
        File.WriteAllText(body, "public partial class Clock { public static long Read() => System.Diagnostics.Stopwatch.GetTimestamp(); }");
        modules.Add(0, "Game", "public partial class Clock { }");
        modules.Idle();
        Assert.Equal(MessageKind.Reload, modules.Sink.Next().Kind);
        var good = modules.Server.Dependency("Game");
        modules.Edit(0, "Game", "public partial class Clock", "[Simulation] public partial class Clock");
        var rejected = modules.Sink.Next();
        Assert.Equal(MessageKind.Diagnostics, rejected.Kind);
        Assert.Contains("ARA0200", rejected.Diagnostics().Json);
        Assert.Same(good, modules.Server.Dependency("Game"));
        File.WriteAllText(body, "public partial class Clock { public static long Read() => 1; }");
        modules.Server.Change(0, [body], AbiConst.OriginHuman, 0);
        modules.Idle();
        Assert.Equal(MessageKind.Reload, modules.Sink.Next().Kind);
    }

    [Fact]
    public void An_added_overload_rechecks_unchanged_call_sites_for_dynamic_dispatch()
    {
        using var modules = new Modules();
        Directory.CreateDirectory(Path.Combine(modules.Root, "Game"));
        var caller = modules.File("Game", "Caller.cs");
        File.WriteAllText(caller, "public partial class Calls { public static int Existing() => Choose(1).GetHashCode(); }");
        modules.Add(0, "Game", "public partial class Calls { static object Choose(object value) => value; /*new*/ }");
        modules.Idle();
        Assert.Equal(MessageKind.Reload, modules.Sink.Next().Kind);
        var good = modules.Server.Dependency("Game");
        modules.Edit(0, "Game", "/*new*/", "static dynamic Choose(int value) => value;");
        var rejected = modules.Sink.Next();
        Assert.Equal(MessageKind.Diagnostics, rejected.Kind);
        Assert.Contains("ARA0400", rejected.Diagnostics().Json);
        Assert.Same(good, modules.Server.Dependency("Game"));
        File.WriteAllText(caller, "public partial class Calls { public static int Existing() => 1; }");
        modules.Server.Change(0, [caller], AbiConst.OriginHuman, 0);
        modules.Idle();
        Assert.Equal(MessageKind.Delta, modules.Sink.Next().Kind);
    }

    [Fact]
    public void A_base_type_edit_rechecks_fields_in_an_unchanged_partial_file()
    {
        using var modules = new Modules();
        Directory.CreateDirectory(Path.Combine(modules.Root, "Game"));
        var fields = modules.File("Game", "Fields.cs");
        File.WriteAllText(fields, "public sealed partial class Actor { int stored; }");
        const string source = "/// <summary>A reloadable helper.</summary>\npublic sealed partial class Actor { }";
        modules.Add(0, "Game", source);
        modules.Idle();
        Assert.Equal(MessageKind.Reload, modules.Sink.Next().Kind);
        var good = modules.Server.Dependency("Game");
        Assert.NotNull(good);
        modules.Edit(0, "Game", "class Actor {", "class Actor : Behaviour {");
        var rejected = modules.Sink.Next();
        Assert.Equal(MessageKind.Diagnostics, rejected.Kind);
        Assert.Contains("ARA0101", rejected.Diagnostics().Json);
        Assert.Same(good, modules.Server.Dependency("Game"));
        File.WriteAllText(fields, "public sealed partial class Actor { [Transient] int stored; }");
        modules.Server.Change(0, [fields], AbiConst.OriginHuman, 0);
        modules.Idle();
        Assert.Equal(MessageKind.Reload, modules.Sink.Next().Kind);
    }
}
