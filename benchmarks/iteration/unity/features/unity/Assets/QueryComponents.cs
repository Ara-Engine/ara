using Unity.Entities;

namespace AraFeatures
{
    /// <summary>Stable logical identity included in every exported record.</summary>
    public struct StableId : IComponentData
    {
        /// <summary>Contiguous logical ID, independent of Entity allocation order.</summary>
        public uint Value;
    }

    /// <summary>Authoritative integer payload and update count.</summary>
    public struct FeatureValue : IComponentData
    {
        /// <summary>Unsigned wrapping arithmetic state.</summary>
        public uint Value;
        /// <summary>Number of selected updates performed on this entity.</summary>
        public uint Updates;
    }

    /// <summary>Required query tag, corresponding to ID bit zero.</summary>
    public struct TagA : IComponentData { }
    /// <summary>Excluded query tag, corresponding to ID bit one.</summary>
    public struct TagB : IComponentData { }
    /// <summary>Archetype diversity tag for ID bit two.</summary>
    public struct TagC : IComponentData { }
    /// <summary>Archetype diversity tag for ID bit three.</summary>
    public struct TagD : IComponentData { }
    /// <summary>Archetype diversity tag for ID bit four.</summary>
    public struct TagE : IComponentData { }
    /// <summary>Archetype diversity tag for ID bit five.</summary>
    public struct TagF : IComponentData { }

    /// <summary>Scalar integer contract shared with the independent Python oracle.</summary>
    public static class MathContract
    {
        /// <summary>Deterministic unsigned mixer, with no external RNG state.</summary>
        public static uint Mix(uint value)
        {
            unchecked
            {
                value = (value ^ (value >> 16)) * 0x7feb352du;
                value = (value ^ (value >> 15)) * 0x846ca68bu;
                return value ^ (value >> 16);
            }
        }
    }
}
