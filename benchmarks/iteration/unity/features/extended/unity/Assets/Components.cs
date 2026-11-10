using System;
using Unity.Entities;

namespace AraExtended
{
    /// <summary>Logical stable identity independent of physical chunk location.</summary>
    public struct Id : IComponentData { /// <summary>Dense ID in 0..count.</summary>
        public uint Value; }
    /// <summary>Authoritative payload and number of logical updates.</summary>
    public struct Value : IComponentData { /// <summary>Wrapping integer payload.</summary>
        public uint Word; /// <summary>Completed logical updates.</summary>
        public uint Updates; }
    /// <summary>Explicit per-entity epoch shared by both change-filter implementations.</summary>
    public struct Epoch : IComponentData { /// <summary>Last selected tick plus one.</summary>
        public uint Value; }
    /// <summary>DAG first stage output.</summary>
    public struct StageA : IComponentData { /// <summary>Stage payload.</summary>
        public uint Value; }
    /// <summary>DAG first independent branch.</summary>
    public struct StageB : IComponentData { /// <summary>Stage payload.</summary>
        public uint Value; }
    /// <summary>DAG second independent branch.</summary>
    public struct StageC : IComponentData { /// <summary>Stage payload.</summary>
        public uint Value; }
    /// <summary>Table/archetype tag added and removed through command playback.</summary>
    public struct TagD : IComponentData { }
    /// <summary>Real ECS dynamic buffer with four inline words, then external storage.</summary>
    [InternalBufferCapacity(4)]
    public struct Word : IBufferElementData { /// <summary>Authoritative buffer element.</summary>
        public uint Value; }

    /// <summary>Immutable per-run inputs; never authoritative gameplay payload.</summary>
    [Serializable]
    public sealed class Config
    {
        /// <summary>Selected percentage in 0..100.</summary>
        public int percent;
        /// <summary>Spread IDs or a rotating clustered interval.</summary>
        public string distribution;
        /// <summary>Initial fixed-buffer live length.</summary>
        public int length;
        /// <summary>Whether resize also trims capacity; measured separately.</summary>
        public bool trim;
        /// <summary>Fork/join DAG instead of a serial dependency chain.</summary>
        public bool fork_join;
    }

    /// <summary>Exact wrapping-u32 arithmetic shared with the written contract.</summary>
    public static class MathContract
    {
        /// <summary>Deterministic initialization/mixing without runtime RNG.</summary>
        public static uint Mix(uint x) { unchecked { x = (x ^ (x >> 16)) * 0x7feb352du; x = (x ^ (x >> 15)) * 0x846ca68bu; return x ^ (x >> 16); } }
        /// <summary>One logical payload update.</summary>
        public static uint Update(uint value, uint id, uint tick) { unchecked { return value * 1664525u + (1013904223u ^ Mix(id + tick)); } }
        /// <summary>Selected explicit epoch; clustered multiplication uses 64-bit arithmetic.</summary>
        public static bool Selected(uint id, uint tick, uint count, uint percent, bool clustered)
        {
            if (!clustered) return id % 100 < percent;
            ulong start = (ulong)tick * 997 % count;
            return ((ulong)id + count - start) % count < (ulong)count * percent / 100;
        }
        /// <summary>Initial or regrown buffer word.</summary>
        public static uint InitialWord(uint id, int index) { unchecked { return Mix((id * 64 + (uint)index) ^ 0x6d2b79f5u); } }
    }
}
