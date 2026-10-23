namespace Ara;

/// <summary>
/// Deterministic random numbers (PCG-XSH-RR). Seeds come only from the context: the
/// session seed, the tick, and a stream key such as the system and entity, so results
/// are identical across machines, thread counts and chunk splits. Never use
/// <c>System.Random</c> in simulation code (ARA0200).
/// </summary>
public struct Rng
{
    ulong _state;
    readonly ulong _inc;

    /// <summary>A generator for one stream of one seed.</summary>
    public Rng(ulong seed, ulong stream)
    {
        _inc = (stream << 1) | 1UL;
        _state = 0;
        NextUInt();
        _state += seed;
        NextUInt();
    }

    /// <summary>
    /// The stream for one entity in one system on one tick: independent of iteration
    /// order and thread count.
    /// </summary>
    public static Rng For(in Tick tick, uint key, Entity entity) =>
        new(Mix(tick.Seed ^ Mix(tick.Frame)), Mix(((ulong)key << 32) ^ entity.Bits));

    /// <summary>The next 32 random bits.</summary>
    public uint NextUInt()
    {
        ulong old = _state;
        _state = unchecked(old * 6364136223846793005UL + _inc);
        uint xorshifted = (uint)(((old >> 18) ^ old) >> 27);
        int rot = (int)(old >> 59);
        return (xorshifted >> rot) | (xorshifted << (-rot & 31));
    }

    /// <summary>A float in <c>[0, 1)</c>.</summary>
    public float NextFloat() => (NextUInt() >> 8) * (1f / (1 << 24));

    /// <summary>A float in <c>[min, max)</c>.</summary>
    public float Range(float min, float max) => min + (max - min) * NextFloat();

    /// <summary>An integer in <c>[min, max)</c> (unbiased).</summary>
    public int Range(int min, int max)
    {
        if (max <= min) return min;
        uint span = (uint)(max - min);
        uint threshold = (uint)(-span % span);
        uint r;
        do r = NextUInt(); while (r < threshold);
        return min + (int)(r % span);
    }

    /// <summary>True with probability <paramref name="p"/>.</summary>
    public bool Chance(float p) => NextFloat() < p;

    /// <summary>SplitMix64 finalizer: spreads correlated keys into independent seeds.</summary>
    static ulong Mix(ulong z)
    {
        z = unchecked((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9UL);
        z = unchecked((z ^ (z >> 27)) * 0x94D049BB133111EBUL);
        return z ^ (z >> 31);
    }
}
