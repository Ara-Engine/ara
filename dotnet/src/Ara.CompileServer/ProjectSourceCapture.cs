using System.Buffers;
using Ara.Hosting;

namespace Ara.CompileServer;

/// <summary>Read-time texts and derived hashes owned only by this project request.</summary>
internal sealed class ProjectSourceCapture
{
    internal sealed record Row(SourceFile Source, string Path, string Text, string Hash);
    internal IReadOnlyList<Row> Sources { get; }
    ProjectSourceCapture(Row[] owned) => Sources = Array.AsReadOnly(owned);

    /// <summary>Fully reads every current file; prior text is reusable only after exact decoded EOF equality.</summary>
    internal static ProjectSourceCapture Read(string project, SourceFile[] files, Func<string, string?>? previousText = null) => new(files.Select(source =>
    {
        var path = Path.Combine(project, source.StoragePath);
        var prior = previousText?.Invoke(path);
        var text = prior is null ? File.ReadAllText(path) : ReadText(path, prior);
        return new Row(source, path, text, PublishedSnapshot.TextHash(text));
    }).ToArray());

    static string ReadText(string path, string previous)
    {
        // Preserve File.ReadAllText's read-only sharing and decoder defaults.
        // Pool read-ahead instead of allocating another private FileStream buffer.
        using var file = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read, bufferSize: 1);
        using var buffered = new PooledReadAheadStream(file, ArrayPool<byte>.Shared);
        using var reader = new StreamReader(buffered);
        return ModuleSource.ReadDecoded(reader, previous, ArrayPool<char>.Shared);
    }
}
