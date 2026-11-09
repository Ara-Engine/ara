using System.Text;

namespace Ara.CompileServer.Tests;

public sealed class SourceReadTests
{
    [Theory]
    [InlineData(0)]
    [InlineData(1)]
    [InlineData(2)]
    [InlineData(3)]
    [InlineData(4)]
    [InlineData(5)]
    public void Equal_decoded_text_reuses_the_prior_string_for_each_supported_bom(int kind)
    {
        var encoding = kind switch
        {
            0 => (Encoding)new UTF8Encoding(false),
            1 => new UTF8Encoding(true),
            2 => new UnicodeEncoding(false, true),
            3 => new UnicodeEncoding(true, true),
            4 => new UTF32Encoding(false, true),
            _ => new UTF32Encoding(true, true),
        };
        var path = Path.Combine(InstalledTools.Scratch("source-encoding"), "Code.cs");
        var prior = new string('a', 4095) + "\U0001f600 café e\u0301\r\n";
        File.WriteAllBytes(path, [.. encoding.GetPreamble(), .. encoding.GetBytes(prior)]);
        Assert.Same(prior, ModuleSource.ReadWithRetry(path, prior));
        Assert.Equal(ModuleSession.ReadWithRetry(path), ModuleSource.ReadWithRetry(path, "old"));
    }

    [Theory]
    [InlineData(4095)]
    [InlineData(4096)]
    [InlineData(4097)]
    public void Changed_units_at_buffer_boundaries_and_shorter_prefixes_are_read_exactly(int boundary)
    {
        var path = Path.Combine(InstalledTools.Scratch("source-boundary"), "Code.cs");
        var prior = new string('x', 8193);
        var text = prior[..boundary] + "changed\r\n";
        File.WriteAllText(path, text, new UTF8Encoding(false));
        var stamp = File.GetLastWriteTimeUtc(path);
        Assert.Equal(text, ModuleSource.ReadWithRetry(path, prior));
        File.WriteAllText(path, prior[..boundary], new UTF8Encoding(false));
        File.SetLastWriteTimeUtc(path, stamp);
        Assert.Equal(prior[..boundary], ModuleSource.ReadWithRetry(path, prior));
        File.Delete(path);
        Assert.Null(ModuleSource.ReadWithRetry(path, prior));
    }

    [Fact]
    public void Invalid_utf8_uses_the_same_decoder_fallback_as_the_original_reader()
    {
        var path = Path.Combine(InstalledTools.Scratch("source-invalid"), "Code.cs");
        File.WriteAllBytes(path, [0x61, 0xc3, 0x28, 0xff, 0xf0, 0x80, 0x80]);
        var expected = ModuleSession.ReadWithRetry(path)!;
        Assert.Same(expected, ModuleSource.ReadWithRetry(path, expected));
        Assert.Equal(expected, ModuleSource.ReadWithRetry(path, "different"));
    }

    [Fact]
    public void A_missed_watcher_notice_with_restored_mtime_still_rejects_a_broken_companion_file()
    {
        using var fixture = new AnalysisModule();
        fixture.Start();
        var good = fixture.Module.State;
        var stamp = File.GetLastWriteTimeUtc(fixture.Paths[0]);
        fixture.Write(0, AnalysisModule.First.Replace("p.Value += 1;", "p.Value += Missing;", StringComparison.Ordinal));
        File.SetLastWriteTimeUtc(fixture.Paths[0], stamp);
        fixture.Write(1, AnalysisModule.Second.Replace("p.Value += 2;", "p.Value += 3;", StringComparison.Ordinal));
        var rejected = fixture.Compile(1);
        Assert.Equal(OutcomeKind.Errors, rejected.Kind);
        Assert.Contains(rejected.Problems, diagnostic => diagnostic.Id == "CS0103");
        Assert.Same(good, fixture.Module.State);
        fixture.Write(0, AnalysisModule.First);
        var repaired = fixture.Compile(1);
        Assert.Equal(OutcomeKind.Delta, repaired.Kind);
    }

    [Fact]
    public void An_encoding_only_save_is_a_noop_for_the_running_compilation()
    {
        using var fixture = new AnalysisModule();
        fixture.Start();
        var good = fixture.Module.State;
        File.WriteAllText(fixture.Paths[0], AnalysisModule.First, new UnicodeEncoding(false, true));
        var saved = fixture.Compile(0);
        Assert.Equal(OutcomeKind.None, saved.Kind);
        Assert.Same(good, saved.Next);
        Assert.True(saved.ValidatedInputs);
    }
}
