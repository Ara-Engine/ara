using System.Reflection.PortableExecutable;

namespace Ara.CompileServer.Tests;

/// <summary>What the compile server's ReadyToRun build step leaves on disk (see Ara.CompileServer.csproj).</summary>
public sealed class ReadyToRunBuildTests
{
    static string DotnetRoot()
    {
        for (var dir = new DirectoryInfo(AppContext.BaseDirectory); dir is not null; dir = dir.Parent)
            if (File.Exists(Path.Combine(dir.FullName, "Ara.slnx"))) return dir.FullName;
        throw new InvalidOperationException("Ara.slnx not found above the test output");
    }

    /// <summary>Build and publish output belongs under artifacts/; a mis-joined path once recreated an absolute path inside the project.</summary>
    [Fact]
    public void The_build_leaves_no_directory_inside_the_project_folder()
    {
        var project = Path.Combine(DotnetRoot(), "src", "Ara.CompileServer");
        Assert.Empty(Directory.GetDirectories(project));
    }

    [Fact]
    public void The_built_server_ships_precompiled_roslyn_unless_the_build_opted_out()
    {
        var output = Path.Combine(DotnetRoot(), "artifacts", "bin", "Ara.CompileServer", "release");
        if (!Directory.Exists(output)) return; // the server was built elsewhere (a private SDK layout)
        // -p:AraReadyToRun=false restores the no-tiering configuration; that build is IL only.
        if (File.ReadAllText(Path.Combine(output, "Ara.CompileServer.runtimeconfig.json")).Contains("\"System.Runtime.TieredCompilation\": false"))
            return;
        foreach (var name in new[] { "Microsoft.CodeAnalysis.dll", "Microsoft.CodeAnalysis.CSharp.dll", "Ara.CompileServer.dll" })
        {
            using var stream = File.OpenRead(Path.Combine(output, name));
            using var pe = new PEReader(stream);
            Assert.True(pe.PEHeaders.CorHeader!.ManagedNativeHeaderDirectory.Size > 0, $"{name} is IL only");
        }
    }

    /// <summary>
    /// The server runs non-concurrent workstation GC: with background GC the gen0 budget is so small that an L3 compile spent a
    /// third of its time in GC pauses (benchmarks/iteration/SERVER_GC.md).
    /// </summary>
    [Fact]
    public void The_built_server_uses_non_concurrent_gc()
    {
        var output = Path.Combine(DotnetRoot(), "artifacts", "bin", "Ara.CompileServer", "release");
        if (!Directory.Exists(output)) return; // the server was built elsewhere (a private SDK layout)
        Assert.Contains("\"System.GC.Concurrent\": false", File.ReadAllText(Path.Combine(output, "Ara.CompileServer.runtimeconfig.json")));
    }
}
