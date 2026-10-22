using System.Runtime.InteropServices;

namespace S8;

/// Blittable float4 matching glam::Vec4 and Unity.Mathematics.float4.
[StructLayout(LayoutKind.Sequential)]
public struct F4(float x, float y, float z, float w)
{
    public float x = x, y = y, z = z, w = w;
    public static F4 operator +(F4 a, F4 b) => new(a.x + b.x, a.y + b.y, a.z + b.z, a.w + b.w);
    public static F4 operator -(F4 a, F4 b) => new(a.x - b.x, a.y - b.y, a.z - b.z, a.w - b.w);
    public static F4 operator *(F4 a, float b) => new(a.x * b, a.y * b, a.z * b, a.w * b);
    public static F4 operator /(F4 a, float b) => new(a.x / b, a.y / b, a.z / b, a.w / b);
}

[StructLayout(LayoutKind.Sequential)]
public struct ChurnCommand
{
    public uint slot, id;
    public F4 p, v, q;
}

[StructLayout(LayoutKind.Sequential)]
public unsafe struct HostView
{
    public F4* p;
    public F4* v;
    public F4* q;
    public F4* old_p;
    public F4* old_v;
    public int* offsets;
    public int* ids;
    public int* cursor;
    public ChurnCommand* commands;
    public int count;
}

/// The native host owns all arrays and the ECS; managed spans never escape a tick.
public static partial class Host
{
    [LibraryImport("s8_host", EntryPoint = "s8_alloc")]
    public static partial nint Allocate(int count, [MarshalAs(UnmanagedType.U1)] bool churn);
    [LibraryImport("s8_host", EntryPoint = "s8_view")]
    public static partial HostView View(nint handle);
    [LibraryImport("s8_host", EntryPoint = "s8_playback")]
    public static partial void Playback(nint handle);
    [LibraryImport("s8_host", EntryPoint = "s8_collect")]
    public static partial void Collect(nint handle);
    [LibraryImport("s8_host", EntryPoint = "s8_free")]
    public static partial void Free(nint handle);
}
