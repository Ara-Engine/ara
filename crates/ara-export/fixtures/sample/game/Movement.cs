namespace Ara.Game;

/// <summary>
/// Gravity, integration and a ground bounce over Rust-owned columns, one call per batch of chunks.
/// Mirrors <c>movement_reference</c> in <c>src/runtime.rs</c> operation for operation, so the
/// host can demand a bit-exact checksum match on every platform and lane.
/// </summary>
public static unsafe class Movement
{
    /// <summary>Advances every row by one fixed tick.</summary>
    public static void Run(Chunk* chunks, uint count, Tick* tick)
    {
        float dt = tick->Dt;
        float g = 9.81f * dt;
        for (uint c = 0; c < count; c++)
        {
            var t = new Span<Transform>(chunks[c].Col0, (int)chunks[c].Len);
            var v = new Span<Velocity>(chunks[c].Col1, (int)chunks[c].Len);
            for (int i = 0; i < t.Length; i++)
            {
                ref var tr = ref t[i];
                ref var ve = ref v[i];
                ve.Y -= g;
                tr.X += ve.X * dt;
                tr.Y += ve.Y * dt;
                tr.Z += ve.Z * dt;
                if (tr.Y < 0f)
                {
                    tr.Y = -tr.Y;
                    ve.Y = -ve.Y * 0.8f;
                }
                tr.Yaw += ve.YawRate * dt;
            }
        }
    }
}

/// <summary>Entry point only for the R2R lane's self-contained layout. The template hosts the runtime; this never runs.</summary>
public static class Program
{
    /// <summary>Refuses to run standalone.</summary>
    public static int Main()
    {
        Console.Error.WriteLine("Game is hosted by ara-player; run that instead.");
        return 1;
    }
}
