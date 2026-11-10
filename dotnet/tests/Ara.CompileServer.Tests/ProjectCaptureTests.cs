using System.Buffers;
using System.Text;
using Ara.Hosting;

namespace Ara.CompileServer.Tests;

public sealed class ProjectCaptureTests
{
    [Theory]
    [InlineData(false)]
    [InlineData(true)]
    public void A_failure_after_reading_a_block_returns_the_comparison_buffer(bool changed)
    {
        var pool = new ComparisonPool();
        using var reader = new FailingReader(changed ? 'b' : 'a');
        Assert.Throws<IOException>(() => ModuleSource.ReadDecoded(reader, new string('a', 8192), pool));
        Assert.Equal(2, reader.Reads);
        Assert.Equal(1, pool.Rents);
        Assert.Equal(1, pool.Returns);
    }

    [Theory]
    [InlineData(0)]
    [InlineData(1)]
    [InlineData(2)]
    [InlineData(3)]
    [InlineData(4)]
    [InlineData(5)]
    public void Reused_project_text_matches_File_ReadAllText_for_every_bom_and_after_same_stamp_edits(int kind)
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
        var root = InstalledTools.Scratch("project-capture-encoding");
        var path = Path.Combine(root, "代码🦀.cs");
        var text = new string('a', 4095) + "🦀 café e\u0301\r\n";
        File.WriteAllText(path, text, encoding);
        var stamp = File.GetLastWriteTimeUtc(path);
        SourceFile[] files = [new("代码🦀.cs", "代码🦀.cs", "game", false)];
        ProjectSourceCapture.Row Read(string prior) => Assert.Single(ProjectSourceCapture.Read(root, files, _ => prior).Sources);
        Assert.Same(text, Read(text).Text);
        foreach (var changed in new[] { text[..4095] + "changed", text[..4096], text + "longer", "" })
        {
            File.WriteAllText(path, changed, encoding);
            File.SetLastWriteTimeUtc(path, stamp);
            var row = Read(text);
            Assert.Equal(File.ReadAllText(path), row.Text);
            Assert.Equal(PublishedSnapshot.TextHash(row.Text), row.Hash);
        }
    }

    [Fact]
    public void Reused_project_text_preserves_invalid_utf8_fallback_and_cannot_hide_missing_or_locked_files()
    {
        var root = InstalledTools.Scratch("project-capture-errors");
        var path = Path.Combine(root, "Code.cs");
        SourceFile[] files = [new("Code.cs", "Code.cs", "game", false)];
        File.WriteAllBytes(path, [0x61, 0xc3, 0x28, 0xff, 0xf0, 0x80, 0x80]);
        var expected = File.ReadAllText(path);
        ProjectSourceCapture.Row Read() => Assert.Single(ProjectSourceCapture.Read(root, files, _ => expected).Sources);
        Assert.Same(expected, Read().Text);
        using (var locked = new FileStream(path, FileMode.Open, FileAccess.ReadWrite, FileShare.None))
            Assert.Throws<IOException>(() => Read());
        // Both successful and failed reads close their handles.
        using (var exclusive = new FileStream(path, FileMode.Open, FileAccess.ReadWrite, FileShare.None)) { }
        File.Delete(path);
        Assert.Throws<FileNotFoundException>(() => Read());
    }

    [Fact]
    public void Preparation_reuses_only_active_module_text_and_rechecks_same_length_same_stamp_content()
    {
        var root = InstalledTools.Scratch("project-capture-retention");
        var path = Path.Combine(root, "Code.cs");
        var output = Path.Combine(root, "game.dll");
        var text = "public static class Value { public static int Read() => 1; }";
        File.WriteAllText(path, text);
        var stamp = File.GetLastWriteTimeUtc(path);
        SourceFile[] files = [new("Code.cs", "Code.cs", "game", false)];
        var cache = new ProjectCompilations();
        var compiler = new ModuleCompiler("game", [typeof(Behaviour).Assembly.Location]);
        var first = Assert.Single(cache.ReadSources(output, root, files).Sources);
        cache.Generate(output, "config", compiler, false, [(path, first.Text)], []);
        var reused = Assert.Single(cache.ReadSources(output, root, files).Sources);
        Assert.Same(first.Text, reused.Text);
        File.WriteAllText(path, text.Replace("=> 1;", "=> X;", StringComparison.Ordinal));
        File.SetLastWriteTimeUtc(path, stamp);
        var changed = Assert.Single(cache.ReadSources(output, root, files).Sources);
        Assert.NotEqual(first.Text, changed.Text);
        Assert.NotEqual(first.Hash, changed.Hash);
        var generated = cache.Generate(output, "config", compiler, false, [(path, changed.Text)], []);
        Assert.Contains(ModuleCompiler.Emit(generated.Final).Errors, diagnostic => diagnostic.Id == "CS0103");
        File.WriteAllText(path, text);
        File.SetLastWriteTimeUtc(path, stamp);
        Assert.Equal(first.Text, Assert.Single(cache.ReadSources(output, root, files).Sources).Text);
        cache.Retain([]);
        var evicted = Assert.Single(cache.ReadSources(output, root, files).Sources);
        Assert.Equal(first.Text, evicted.Text);
        Assert.NotSame(first.Text, evicted.Text);
    }

    sealed class ComparisonPool : ArrayPool<char>
    {
        internal int Rents, Returns;
        public override char[] Rent(int minimumLength) { Rents++; return new char[minimumLength]; }
        public override void Return(char[] array, bool clearArray = false) => Returns++;
    }

    sealed class FailingReader(char unit) : TextReader
    {
        internal int Reads;
        public override int Read(Span<char> buffer)
        {
            if (++Reads == 2) throw new IOException("injected mid-read failure");
            buffer.Fill(unit);
            return buffer.Length;
        }
    }
}
