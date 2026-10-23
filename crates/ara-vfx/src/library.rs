//! Complete module search metadata for the native stack inspector and agents.
use crate::Stage;

/// A documented kind, independent of a particular effect instance.
pub struct ModuleKind {
    /// Registry key after the `vfx.` prefix.
    pub name: &'static str,
    /// The only legal stack for this module.
    pub stage: Stage,
    /// Inspector tooltip and agent documentation.
    pub doc: &'static str,
}
macro_rules! kinds {($(($name:literal,$stage:ident,$doc:literal)),* $(,)?)=>{
    /// Every built-in module in stack/library order; settings are typed Module variants.
    pub const MODULES:&[ModuleKind]=&[$(ModuleKind{name:$name,stage:Stage::$stage,doc:$doc}),*];
};}
kinds![
    ("rate",Spawn,"Emit particles per second with fractional fixed-tick accumulation."),
    ("burst",Spawn,"Emit an exact count once at a fixed tick."),
    ("lifetime",Initialize,"Set lifetime in seconds, diameter in metres and linear RGBA."),
    ("point",Initialize,"Initialize at a world-space point, relative to incoming events."),
    ("sphere",Initialize,"Sample a uniform spherical volume using the particle seed."),
    ("box",Initialize,"Sample a uniform box using centre and half extents."),
    ("cone",Initialize,"Sample an upward cone volume using tip, height and radius."),
    ("mesh_surface",Initialize,"Sample cooked triangles by area and barycentric position."),
    ("velocity",Initialize,"Set metres-per-second velocity and symmetric per-axis spread."),
    ("gravity",Update,"Add acceleration for the next tick's displacement."),
    ("drag",Update,"Apply stable rational velocity damping in inverse seconds."),
    ("curl_noise",Update,"Apply divergence-free analytic curl acceleration."),
    ("vortex",Update,"Accelerate tangentially around an axis through a centre."),
    ("attractor",Update,"Apply softened inverse-square attraction toward a point."),
    ("color_over_life",Update,"Evaluate 2–16 normalized-time linear RGBA gradient keys."),
    ("size_over_life",Update,"Evaluate 2–16 normalized-time world-diameter curve keys."),
    ("plane_collision",Update,"Project out of a plane and reflect inward velocity."),
    ("depth_collision",Update,"Collide against reversed-Z depth or a CPU DepthQuery."),
    ("sprite",Render,"Draw soft camera-facing sprites through surface, unlit or UI materials."),
    ("flipbook",Render,"Address a grid atlas by particle age and frames per second."),
    ("ribbon",Render,"Join live particles by age into camera-facing ribbon segments."),
    ("mesh",Render,"Draw velocity-oriented cooked triangles with optional sorting."),
    ("light",Render,"Emit a capped emitter light with linear tint, lumens and radius."),
    ("event_spawn",Render,"Route collision/death positions to a named emitter next tick."),
];
