using System.Runtime.InteropServices;

namespace Ara;

/// <summary>A linear RGBA color (the schema's <c>color</c> type). Defaults to opaque white in the schema.</summary>
[StructLayout(LayoutKind.Sequential)]
public struct Color : IEquatable<Color>
{
    /// <summary>Red, linear.</summary>
    public float R;
    /// <summary>Green, linear.</summary>
    public float G;
    /// <summary>Blue, linear.</summary>
    public float B;
    /// <summary>Alpha (1 is opaque).</summary>
    public float A;

    /// <summary>Creates a color from linear components.</summary>
    public Color(float r, float g, float b, float a = 1f) => (R, G, B, A) = (r, g, b, a);

    /// <summary>Opaque white.</summary>
    public static Color White => new(1f, 1f, 1f);

    /// <inheritdoc/>
    public readonly bool Equals(Color other) => R == other.R && G == other.G && B == other.B && A == other.A;

    /// <inheritdoc/>
    public override readonly bool Equals(object? obj) => obj is Color c && Equals(c);

    /// <inheritdoc/>
    public override readonly int GetHashCode() => HashCode.Combine(R, G, B, A);
}
