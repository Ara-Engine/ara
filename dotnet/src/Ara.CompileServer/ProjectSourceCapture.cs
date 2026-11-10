using Ara.Hosting;

namespace Ara.CompileServer;

/// <summary>Read-time texts and derived hashes owned only by this project request.</summary>
internal sealed class ProjectSourceCapture
{
    internal sealed record Row(SourceFile Source, string Path, string Text, string Hash);
    internal IReadOnlyList<Row> Sources { get; }
    ProjectSourceCapture(Row[] owned) => Sources = Array.AsReadOnly(owned);

    /// <summary>Fully reads every current file; callers cannot supply a claimed text hash.</summary>
    internal static ProjectSourceCapture Read(string project, SourceFile[] files) => new(files.Select(source =>
    {
        var path = Path.Combine(project, source.StoragePath);
        var text = File.ReadAllText(path);
        return new Row(source, path, text, PublishedSnapshot.TextHash(text));
    }).ToArray());
}
