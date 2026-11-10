namespace Ara.CompileServer.Tests;

public sealed class ProjectEmissionTests
{
    [Theory]
    [InlineData("public static class Broken { public static int Value => Missing; }", "CS0103")]
    [InlineData("using Ara; [Component] public partial struct Undocumented { public int Value; }", "ARA0703")]
    [InlineData("public static class InvalidState { public static int Value; }", "ARA0100")]
    public void Compile_failures_do_not_publish_an_image(string source, string code)
    {
        var root = InstalledTools.Scratch("project-emission-error");
        Directory.CreateDirectory(Path.Combine(root, "content"));
        File.WriteAllText(Path.Combine(root, "content", "Code.cs"), source);
        var result = ProjectCompiler.Compile(new(root, "content"));
        Assert.Contains(result.Diagnostics, d => ((string?)d["message"])?.Contains(code, StringComparison.Ordinal) == true || (string?)d["code"] == code);
        var module = Assert.Single(result.Modules);
        Assert.Equal("failed", module.Result);
        Assert.Null(module.Schema);
        Assert.False(File.Exists(Path.Combine(root, module.AssemblyPath)));
    }
}
