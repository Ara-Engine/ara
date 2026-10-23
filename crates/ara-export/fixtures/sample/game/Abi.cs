using System.Runtime.InteropServices;

namespace Ara.Game;

/// <summary>ABI constants shared with <c>src/abi.rs</c>. Hand-mirrored in the spike; generated in Ara.</summary>
public static class Abi
{
    /// <summary>Bumped on any layout change; both sides refuse to bind on mismatch.</summary>
    public const uint Version = 1;
}

/// <summary>Position plus yaw: the column <see cref="Movement"/> writes.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct Transform
{
    /// <summary>World-space position in metres.</summary>
    public float X, Y, Z;
    /// <summary>Heading in radians.</summary>
    public float Yaw;
}

/// <summary>Linear velocity plus yaw rate.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct Velocity
{
    /// <summary>Metres per second.</summary>
    public float X, Y, Z;
    /// <summary>Radians per second.</summary>
    public float YawRate;
}

/// <summary>One archetype chunk of Rust-owned memory: <c>Len</c> rows of two columns.</summary>
[StructLayout(LayoutKind.Sequential)]
public unsafe struct Chunk
{
    /// <summary>Rows in this chunk.</summary>
    public uint Len;
    /// <summary>Valid column pointers.</summary>
    public uint ColumnCount;
    /// <summary><see cref="Transform"/> column.</summary>
    public byte* Col0;
    /// <summary><see cref="Velocity"/> column.</summary>
    public byte* Col1;
}

/// <summary>The fixed simulation tick. Never wall-clock.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct Tick
{
    /// <summary>Tick number since start.</summary>
    public ulong Frame;
    /// <summary>Fixed step in seconds.</summary>
    public float Dt;
    /// <summary>Frame times Dt.</summary>
    public float Elapsed;
}

/// <summary>Rust → C# host table, copied at init.</summary>
[StructLayout(LayoutKind.Sequential)]
public unsafe struct HostApi
{
    /// <summary>Must equal <see cref="Abi.Version"/>.</summary>
    public uint AbiVersion;
    /// <summary>Host's sizeof(HostApi).</summary>
    public uint StructSize;
    /// <summary>UTF-8 log line; level 0 debug, 1 info, 2 error.</summary>
    public delegate* unmanaged<uint, byte*, nuint, void> Log;
    /// <summary>Empty callback for C# → Rust call-cost measurement.</summary>
    public delegate* unmanaged<uint, uint> Nop;
}

/// <summary>C# → Rust table, filled by <c>ara_script_init</c>.</summary>
[StructLayout(LayoutKind.Sequential)]
public unsafe struct ScriptApi
{
    /// <summary>C#'s ABI version.</summary>
    public uint AbiVersion;
    /// <summary>C#'s sizeof(ScriptApi).</summary>
    public uint StructSize;
    /// <summary>Runs a system over chunks: 0 ok, 1 faulted (logged), 2 unknown id.</summary>
    public delegate* unmanaged<uint, Chunk*, uint, Tick*, int> RunSystem;
    /// <summary>FNV-1a over the Transform column; also logged.</summary>
    public delegate* unmanaged<Chunk*, uint, ulong> Checksum;
    /// <summary>Empty export for Rust → C# call-cost measurement.</summary>
    public delegate* unmanaged<uint, uint> Nop;
    /// <summary>Calls the host's Nop n times; fast != 0 suppresses the GC transition.</summary>
    public delegate* unmanaged<ulong, uint, ulong> CallHost;
    /// <summary>Loads an IL mod. Status 2 (logged) where code can't be loaded, i.e. NativeAOT.</summary>
    public delegate* unmanaged<byte*, nuint, int> LoadMod;
}
