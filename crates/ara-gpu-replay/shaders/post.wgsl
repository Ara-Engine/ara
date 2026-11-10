// PP1: all filters read reconstructed HDR, never temporal history.
struct Settings {
    bloom_motion:vec4<f32>, // energy, half shutter fraction, max velocity, frame
    lens:vec4<f32>, // focus m, thin-lens CoC scale in pixels, max CoC, DOF enabled
    grade:vec4<f32>, // contrast, saturation, vignette, grain
    white:vec4<f32>, // RGB white-balance gains, filmic
    display:vec4<f32>, // aberration px, LUT strength, LUT size, unused
};
@group(0) @binding(0) var input_colour:texture_2d<f32>;
@group(0) @binding(1) var second_colour:texture_2d<f32>;
@group(0) @binding(2) var auxiliary:texture_2d<f32>;
@group(0) @binding(3) var velocity_tiles:texture_2d<f32>;
@group(0) @binding(4) var output_colour:texture_storage_2d<rgba16float,write>;
@group(0) @binding(5) var<uniform> settings:Settings;
@group(0) @binding(6) var linear_sampler:sampler;

fn load(t:texture_2d<f32>,p:vec2<i32>)->vec4<f32> {
    return textureLoad(t,clamp(p,vec2<i32>(0),vec2<i32>(textureDimensions(t))-1),0);
}
fn bilinear(t:texture_2d<f32>,uv:vec2<f32>)->vec4<f32> {
    return textureSampleLevel(t,linear_sampler,uv,0.0);
}
fn tent(t:texture_2d<f32>,uv:vec2<f32>)->vec4<f32> {
    let d=1.0/vec2<f32>(textureDimensions(t));
    var c=vec4<f32>(0.0);
    for(var y=-1;y<=1;y++) {for(var x=-1;x<=1;x++) {
        let w=f32((2-abs(x))*(2-abs(y)))/16.0;
        c+=bilinear(t,uv+vec2<f32>(f32(x),f32(y))*d)*w;
    }}
    return c;
}
@compute @workgroup_size(8,8) fn downsample(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=textureDimensions(output_colour);if any(id.xy>=size) {return;}
    let uv=(vec2<f32>(id.xy)+0.5)/vec2<f32>(size);
    textureStore(output_colour,id.xy,tent(input_colour,uv));
}
@compute @workgroup_size(8,8) fn upsample(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=textureDimensions(output_colour);if any(id.xy>=size) {return;}
    let uv=(vec2<f32>(id.xy)+0.5)/vec2<f32>(size);
    // Convex combination preserves DC at every level: no additive energy gain.
    textureStore(output_colour,id.xy,mix(load(input_colour,vec2<i32>(id.xy)),tent(second_colour,uv),0.65));
}
@compute @workgroup_size(8,8) fn tile_max(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=textureDimensions(output_colour);if any(id.xy>=size) {return;}
    var best=vec2<f32>(0.0);
    for(var y=0;y<16;y++) {for(var x=0;x<16;x++) {
        let v=load(auxiliary,vec2<i32>(id.xy*16u)+vec2<i32>(x,y)).xy;
        if dot(v,v)>dot(best,best) {best=v;}
    }}
    textureStore(output_colour,id.xy,vec4<f32>(best,0.0,0.0));
}
fn cone(d:f32,r:f32)->f32 {return clamp(1.0-d/max(r,0.001),0.0,1.0);}
fn cylinder(d:f32,r:f32)->f32 {return 1.0-smoothstep(max(0.0,r-1.0),r+1.0,d);}
@compute @workgroup_size(8,8) fn motion_blur(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=textureDimensions(output_colour);if any(id.xy>=size) {return;}
    let p=vec2<i32>(id.xy);let a=load(auxiliary,p);let own=a.xy;
    var v=own;
    let tile=p/16;
    let reach=i32(ceil(settings.bloom_motion.z/16.0));
    for(var y=-reach;y<=reach;y++) {for(var x=-reach;x<=reach;x++) {
        let candidate=load(velocity_tiles,tile+vec2<i32>(x,y)).xy;
        if dot(candidate,candidate)>dot(v,v) {v=candidate;}
    }}
    let radius=length(v);
    let centre=load(input_colour,p);
    if radius<0.5 {textureStore(output_colour,p,centre);return;}
    // McGuire et al. 2012: neighbour max direction, soft depth comparison,
    // foreground/background cones and cylinder overlap. Original implementation.
    var sum=centre.rgb/max(length(own),1.0);var weight=1.0/max(length(own),1.0);
    for(var i=0;i<16;i++) {
        let t=(f32(i)+0.5)/8.0-1.0;
        let q=vec2<f32>(p)+0.5+v*t;
        let b=load(auxiliary,vec2<i32>(q));let d=abs(t)*radius;
        let soft=max(0.02,0.02*min(a.z,b.z));
        let front=clamp(1.0-(b.z-a.z)/soft,0.0,1.0);
        let back=clamp(1.0-(a.z-b.z)/soft,0.0,1.0);
        let w=front*cone(d,length(b.xy))+back*cone(d,length(own))
            +2.0*cylinder(d,length(b.xy))*cylinder(d,length(own));
        sum+=bilinear(input_colour,q/vec2<f32>(size)).rgb*w;weight+=w;
    }
    textureStore(output_colour,p,vec4<f32>(sum/max(weight,1e-5),centre.a));
}
fn coc(z:f32)->f32 {
    return clamp(settings.lens.y*(z-settings.lens.x)/max(z,0.001),-settings.lens.z,settings.lens.z);
}
@compute @workgroup_size(8,8) fn depth_of_field(@builtin(global_invocation_id) id:vec3<u32>) {
    let size=textureDimensions(output_colour);if any(id.xy>=size) {return;}
    let p=vec2<i32>(id.xy);let z=load(auxiliary,p).z;let c=coc(z);
    let centre=load(input_colour,p);
    let foreground=c< -0.5;
    var far_sum=select(centre.rgb,vec3<f32>(0.0),foreground);
    var far_w=select(1.0,0.0,foreground);
    var near_sum=vec3<f32>(0.0);var near_w=0.0;
    let gather_radius=select(settings.lens.z,-c,foreground);
    // A fixed disc gathers both fields, including foreground bokeh beyond its silhouette.
    // Separate near/far weights stop an in-focus foreground from accepting far colour.
    for(var i=0;i<48;i++) {
        let r=sqrt((f32(i)+0.5)/48.0)*gather_radius;
        let angle=f32(i)*2.39996323;
        let q=vec2<f32>(p)+0.5+vec2<f32>(cos(angle),sin(angle))*r;
        let nz=load(auxiliary,vec2<i32>(q)).z;let nc=coc(nz);
        let colour=bilinear(input_colour,q/vec2<f32>(size)).rgb;
        if nc< -0.5 {
            let w=cylinder(r,-nc)/max(nc*nc,1.0);
            near_sum+=colour*w;near_w+=w;
        }
        if c>0.5 && nc>0.0 && nz>=z-max(0.02,z*0.02) {
            let w=cylinder(r,min(c,nc));far_sum+=colour*w;far_w+=w;
        }
        if foreground && nc>= -0.5 {
            // Estimate the background under a blurred foreground boundary. Keeping
            // the sharp foreground centre as background would leave a hard inner edge.
            let w=cylinder(r,-c);far_sum+=colour*w;far_w+=w;
        }
    }
    var far_colour=centre.rgb;
    if far_w>1e-5 {far_colour=far_sum/far_w;}
    let coverage=clamp(near_w*gather_radius*gather_radius/48.0,0.0,1.0);
    textureStore(output_colour,p,vec4<f32>(mix(far_colour,near_sum/max(near_w,1e-6),coverage),centre.a));
}
