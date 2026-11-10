using System.Collections.Immutable;
using Microsoft.CodeAnalysis;

namespace Ara.CompileServer;

/// <summary>
/// Owns one concurrent analyzer request. Even an early emit failure cancels and
/// observes this task before evaluation returns; Roslyn's private workers are not owned here.
/// </summary>
internal sealed class PendingAnalysis : IDisposable
{
    internal const string FailureKey = "Ara.CompileServer.SecondaryAnalysisFailure";
    internal const string DiagnosticsKey = "Ara.CompileServer.SecondaryAnalysisDiagnostics";
    readonly CancellationTokenSource _cancel;
    readonly CancellationToken _parent;
    readonly Task<ImmutableArray<Diagnostic>> _task;
    bool _observed;
    bool _disposed;

    internal PendingAnalysis(Func<CancellationToken, ImmutableArray<Diagnostic>> analyze, CancellationToken parent)
    {
        _parent = parent;
        _cancel = CancellationTokenSource.CreateLinkedTokenSource(parent);
        _task = Task.Run(() => analyze(_cancel.Token), _cancel.Token);
    }

    /// <summary>Secondary failure retained when a primary emit error/cancellation prevented normal observation.</summary>
    internal Exception? Failure { get; private set; }
    /// <summary>Raw Roslyn failure diagnostics retained even when compiler errors skip ordinary analyzer reporting.</summary>
    internal ImmutableArray<Diagnostic> Failures { get; private set; } = [];

    /// <summary>Only cancellation of the supplied, requested token represents this operation yielding.</summary>
    internal static bool IsCancellation(OperationCanceledException error, CancellationToken token) =>
        token.IsCancellationRequested && error.CancellationToken == token;

    /// <summary>Returns the result, translating this child's cancellation back to its caller's requested token.</summary>
    internal ImmutableArray<Diagnostic> Complete()
    {
        try
        {
            var result = _task.GetAwaiter().GetResult();
            if (_parent.IsCancellationRequested)
                Failures = [.. result.Where(diagnostic => diagnostic.Id is "AD0001" or "AD0002")];
            _parent.ThrowIfCancellationRequested();
            return result;
        }
        catch (OperationCanceledException error) when (_parent.IsCancellationRequested && IsCancellation(error, _cancel.Token))
        {
            throw new OperationCanceledException(_parent);
        }
        finally { _observed = true; }
    }

    /// <summary>Cancels unconsumed work and drains without abandoning a wait on cancellation.</summary>
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        try
        {
            if (_observed) return;
            if (!_task.IsCompleted)
            {
                try { _cancel.Cancel(); }
                catch (Exception error) { Record(error); }
            }
            try
            {
                var diagnostics = _task.GetAwaiter().GetResult();
                Failures = [.. diagnostics.Where(diagnostic => diagnostic.Id is "AD0001" or "AD0002")];
            }
            catch (OperationCanceledException error) when (IsCancellation(error, _cancel.Token)) { }
            catch (Exception error) { Record(error); }
        }
        finally { _cancel.Dispose(); }
    }

    void Record(Exception error) => Failure = Failure is null ? error : new AggregateException(Failure, error);
}
