using System.Text;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.Text;

namespace Ara.CompileServer;

/// <summary>Reparses exact file replacements without retaining a chain of edited source buffers.</summary>
internal static class ModuleText
{
    /// <summary>The previous tree and text must describe the same captured file.</summary>
    internal static SyntaxTree Update(string path, SyntaxTree previous, string before, string after)
    {
        if (previous.FilePath != path) return ModuleCompiler.Parse(path, after);
        if (before == after) return previous;
        // Small files cost less to parse afresh than to prepare an incremental edit.
        if (before.Length < 2048 || after.Length < 2048) return ModuleCompiler.Parse(path, after);
        var start = 0;
        while (start < before.Length && start < after.Length && before[start] == after[start]) start++;
        var oldEnd = before.Length;
        var newEnd = after.Length;
        while (oldEnd > start && newEnd > start && before[oldEnd - 1] == after[newEnd - 1]) { oldEnd--; newEnd--; }
        var oldText = previous.GetText();
        var text = SourceText.From(after, oldText.Encoding, oldText.ChecksumAlgorithm);
        return previous.WithChangedText(new FlatChange(text, oldText,
            new TextChangeRange(TextSpan.FromBounds(start, oldEnd), newEnd - start)));
    }

    // Roslyn needs the immediate edit range, not CompositeText's backing buffers
    // and change history. Other ancestors conservatively see a full replacement.
    sealed class FlatChange(SourceText text, SourceText previous, TextChangeRange change)
        : SourceText(checksumAlgorithm: text.ChecksumAlgorithm)
    {
        readonly WeakReference<SourceText> _previous = new(previous);
        public override Encoding? Encoding => text.Encoding;
        public override int Length => text.Length;
        public override char this[int position] => text[position];
        public override void CopyTo(int sourceIndex, char[] destination, int destinationIndex, int count) =>
            text.CopyTo(sourceIndex, destination, destinationIndex, count);
        public override string ToString(TextSpan span) => text.ToString(span);
        public override IReadOnlyList<TextChangeRange> GetChangeRanges(SourceText oldText)
        {
            if (ReferenceEquals(this, oldText)) return [];
            if (_previous.TryGetTarget(out var value) && ReferenceEquals(value, oldText)) return [change];
            return base.GetChangeRanges(oldText);
        }
    }
}
