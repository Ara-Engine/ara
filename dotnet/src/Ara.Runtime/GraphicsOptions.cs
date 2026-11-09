using System.Text.Json;
using System.Text.Json.Serialization;

namespace Ara;

/// <summary>Independent renderer work budgets, cheapest first.</summary>
[JsonConverter(typeof(JsonStringEnumConverter<GraphicsTier>))]
public enum GraphicsTier
{
    /// <summary>Smallest budget.</summary>
    [JsonStringEnumMemberName("low")] Low,
    /// <summary>Balanced budget.</summary>
    [JsonStringEnumMemberName("medium")] Medium,
    /// <summary>Detailed realtime rendering.</summary>
    [JsonStringEnumMemberName("high")] High,
    /// <summary>Largest budget.</summary>
    [JsonStringEnumMemberName("epic")] Epic,
}

/// <summary>Overall preset; Auto uses the GPU's device-local memory.</summary>
[JsonConverter(typeof(JsonStringEnumConverter<GraphicsPreset>))]
public enum GraphicsPreset
{
    /// <summary>Device-based selection. Never enables ray tracing.</summary>
    [JsonStringEnumMemberName("auto")] Auto,
    /// <summary>Low work budgets.</summary>
    [JsonStringEnumMemberName("low")] Low,
    /// <summary>Medium work budgets.</summary>
    [JsonStringEnumMemberName("medium")] Medium,
    /// <summary>High work budgets.</summary>
    [JsonStringEnumMemberName("high")] High,
    /// <summary>Epic work budgets.</summary>
    [JsonStringEnumMemberName("epic")] Epic,
}

/// <summary>Native window presentation policy. Headless/embedded surfaces belong to their owner.</summary>
[JsonConverter(typeof(JsonStringEnumConverter<GraphicsWindowMode>))]
public enum GraphicsWindowMode
{
    /// <summary>Resizable native window.</summary>
    [JsonStringEnumMemberName("windowed")] Windowed,
    /// <summary>Desktop fullscreen without changing the monitor mode.</summary>
    [JsonStringEnumMemberName("borderless")] Borderless,
    /// <summary>Exclusive fullscreen at a supported display mode.</summary>
    [JsonStringEnumMemberName("fullscreen")] Fullscreen,
}

/// <summary>Reconstruction methods available in Ara; temporal antialiasing stays enabled.</summary>
[JsonConverter(typeof(JsonStringEnumConverter<GraphicsUpscaler>))]
public enum GraphicsUpscaler
{
    /// <summary>Inherit the anti-aliasing group.</summary>
    [JsonStringEnumMemberName("preset")] Preset,
    /// <summary>Temporal antialiasing and upsampling.</summary>
    [JsonStringEnumMemberName("taau")] Taau,
    /// <summary>Ara temporal super resolution.</summary>
    [JsonStringEnumMemberName("tsr")] Tsr,
}

/// <summary>Scalability group overrides. Null inherits the overall preset.</summary>
public sealed record GraphicsGroups
{
    /// <summary>Screen-size culling and LOD error.</summary>
    public GraphicsTier? ViewDistance { get; init; }
    /// <summary>Cascades, far maps, local and virtual shadows.</summary>
    public GraphicsTier? Shadows { get; init; }
    /// <summary>Probe relight budget.</summary>
    public GraphicsTier? GlobalIllumination { get; init; }
    /// <summary>SSR and reflection capture budgets.</summary>
    public GraphicsTier? Reflections { get; init; }
    /// <summary>Bloom, AO, depth of field and motion blur budgets.</summary>
    public GraphicsTier? PostProcessing { get; init; }
    /// <summary>Texture streaming residency budget.</summary>
    public GraphicsTier? Textures { get; init; }
    /// <summary>Emissive light budget.</summary>
    public GraphicsTier? Effects { get; init; }
    /// <summary>Stable foliage density: 25/50/75/100 percent.</summary>
    public GraphicsTier? Foliage { get; init; }
    /// <summary>Temporal antialiasing method.</summary>
    public GraphicsTier? AntiAliasing { get; init; }
}

/// <summary>Immutable graphics candidate. Set GraphicsSettings.Pending, then Apply explicitly.</summary>
public sealed record GraphicsOptions
{
    /// <summary>Overall preset. Changing it retains individual overrides.</summary>
    public GraphicsPreset Preset { get; init; }
    /// <summary>Independent scalability groups.</summary>
    public GraphicsGroups Groups { get; init; } = new();
    /// <summary>Render/display scale, 0.1..1. Null inherits the preset.</summary>
    public float? ResolutionScale { get; init; }
    /// <summary>Reconstruction method, or the anti-aliasing group's preset.</summary>
    public GraphicsUpscaler Upscaler { get; init; }
    /// <summary>Allow bloom within the post-processing budget.</summary>
    public bool Bloom { get; init; } = true;
    /// <summary>Allow authored motion blur within the post-processing budget.</summary>
    public bool MotionBlur { get; init; } = true;
    /// <summary>Allow authored depth of field within the post-processing budget.</summary>
    public bool DepthOfField { get; init; } = true;
    /// <summary>Allow ambient occlusion within the post-processing budget.</summary>
    public bool AmbientOcclusion { get; init; } = true;
    /// <summary>Texture residency cap in MiB, 128..32768; null inherits the textures group.</summary>
    public uint? TextureBudgetMib { get; init; }
    /// <summary>Native game window policy.</summary>
    public GraphicsWindowMode WindowMode { get; init; } = GraphicsWindowMode.Borderless;
    /// <summary>Physical window/exclusive pixels, clamped to 320..8192. Borderless uses the desktop extent.</summary>
    public uint Width { get; init; } = 1280;
    /// <summary>Physical window/exclusive pixels, clamped to 180..8192. Borderless uses the desktop extent.</summary>
    public uint Height { get; init; } = 720;
    /// <summary>Exclusive refresh in millihertz; zero uses the desktop/default rate. Other modes retain desktop refresh.</summary>
    public uint RefreshRateMillihz { get; init; }
    /// <summary>Platform monitor ID from Capabilities.Modes; zero keeps the current monitor.</summary>
    public uint Monitor { get; init; }
    /// <summary>FIFO presentation synchronization.</summary>
    public bool Vsync { get; init; } = true;
    /// <summary>Presentation FPS cap, zero for uncapped, otherwise 15..1000. Never changes simulation.</summary>
    public uint FrameRateCap { get; init; }
    /// <summary>Request HDR10 output. Apply rejects it when the surface cannot encode HDR.</summary>
    public bool Hdr { get; init; }
    /// <summary>Explicit hardware RT opt-in, off by default. Save and restart to reopen the device.</summary>
    public bool RayTracing { get; init; }
}

/// <summary>One supported exclusive display mode, in physical pixels.</summary>
public sealed record GraphicsDisplayMode
{
    /// <summary>Stable platform monitor ID.</summary>
    public uint Monitor { get; init; }
    /// <summary>Horizontal pixels.</summary>
    public uint Width { get; init; }
    /// <summary>Vertical pixels.</summary>
    public uint Height { get; init; }
    /// <summary>Refresh rate in millihertz.</summary>
    public uint RefreshRateMillihz { get; init; }
}

/// <summary>Hardware and surface support. Query DeviceKnown before interpreting GPU capabilities.</summary>
public sealed record GraphicsCapabilities
{
    /// <summary>Verified physical GPU ray-query support, independent of current enablement.</summary>
    public bool RayTracingSupported { get; init; }
    /// <summary>True after this player's GPU has been queried.</summary>
    public bool DeviceKnown { get; init; }
    /// <summary>Device-local memory in MiB, or null for unknown/shared memory.</summary>
    public ulong? DeviceLocalMib { get; init; }
    /// <summary>True when this player owns a mutable native display.</summary>
    public bool DisplayAvailable { get; init; }
    /// <summary>The window's current platform monitor ID, or zero without a display.</summary>
    public uint CurrentMonitor { get; init; }
    /// <summary>True only when this surface can present encoded HDR10.</summary>
    public bool HdrSupported { get; init; }
    /// <summary>Supported monitor modes. Empty in headless/embedded players.</summary>
    public GraphicsDisplayMode[] Modes { get; init; } = [];
}

/// <summary>A settings validation or storage diagnostic.</summary>
public sealed record GraphicsDiagnostic
{
    /// <summary>Catalog code, e.g. ARA3500.</summary>
    public string Code { get; init; } = "";
    /// <summary>Diagnostic severity.</summary>
    public string Severity { get; init; } = "";
    /// <summary>Cause or normalized value.</summary>
    public string Message { get; init; } = "";
}

/// <summary>Immutable snapshot of the local user's options and their application status.</summary>
public sealed record GraphicsState
{
    /// <summary>Editable candidate.</summary>
    public GraphicsOptions Pending { get; init; } = new();
    /// <summary>Accepted preferences. The renderer adopts live fields at its next frame boundary.</summary>
    public GraphicsOptions Applied { get; init; } = new();
    /// <summary>Project defaults.</summary>
    public GraphicsOptions Defaults { get; init; } = new();
    /// <summary>Current hardware/display support.</summary>
    public GraphicsCapabilities Capabilities { get; init; } = new();
    /// <summary>Monotonic accepted application number, local to this host.</summary>
    public ulong Revision { get; init; }
    /// <summary>Ray-tracing preference requires Save and a player/device restart.</summary>
    public bool RestartRequired { get; init; }
    /// <summary>Immutable ray-tracing opt-in used to open the current device.</summary>
    public bool ActiveRayTracing { get; init; }
    /// <summary>Validation and persistence notices for this operation.</summary>
    public GraphicsDiagnostic[] Diagnostics { get; init; } = [];
}

