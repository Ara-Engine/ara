struct Settings {
    bloom_motion:vec4<f32>,lens:vec4<f32>,grade:vec4<f32>,white:vec4<f32>,display:vec4<f32>,
};
@group(0) @binding(0) var colour:texture_2d<f32>;
@group(0) @binding(1) var bloom:texture_2d<f32>;
@group(0) @binding(2) var lut:texture_3d<f32>;
@group(0) @binding(3) var linear_sampler:sampler;
@group(0) @binding(4) var<uniform> settings:Settings;
@vertex fn vs(@builtin(vertex_index) id:u32)->@builtin(position) vec4<f32> {
    let p=vec2<f32>(f32((id<<1u)&2u),f32(id&2u));return vec4<f32>(p*2.0-1.0,0.0,1.0);
}
fn hdr(uv:vec2<f32>)->vec3<f32> {
    return mix(textureSampleLevel(colour,linear_sampler,uv,0.0).rgb,
        textureSampleLevel(bloom,linear_sampler,uv,0.0).rgb,settings.bloom_motion.x);
}
fn tone(x:vec3<f32>)->vec3<f32> {
    if settings.white.w>0.5 {
        // Narkowicz's ACES-like fit, CC0 (2016); not a full ACES transform.
        // https://knarkowicz.wordpress.com/2016/01/06/aces-filmic-tone-mapping-curve/
        return clamp((x*(2.51*x+0.03))/(x*(2.43*x+0.59)+0.14),vec3<f32>(0.0),vec3<f32>(1.0));
    }
    return x/(1.0+x);
}
fn hash(p:vec2<u32>,frame:u32)->f32 {
    var h=p.x*0x9e3779b9u ^ p.y*0x85ebca6bu ^ frame*0xc2b2ae35u;
    h=(h^(h>>16u))*0x7feb352du;h=(h^(h>>15u))*0x846ca68bu;
    return f32((h^(h>>16u))>>8u)/16777216.0;
}
@fragment fn fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32> {
    let size=vec2<f32>(textureDimensions(colour));let uv=p.xy/size;
    let radial=uv*2.0-1.0;
    let shift=radial*dot(radial,radial)*settings.display.x/size;
    var x=hdr(uv);
    if settings.display.x>0.0 {x=vec3<f32>(hdr(uv+shift).r,x.g,hdr(uv-shift).b);}
    x=tone(max(x*settings.white.rgb,vec3<f32>(0.0)));
    x=0.18*pow(max(x/0.18,vec3<f32>(0.0)),vec3<f32>(settings.grade.x));
    let l=dot(x,vec3<f32>(0.2126,0.7152,0.0722));
    x=clamp(mix(vec3<f32>(l),x,settings.grade.y),vec3<f32>(0.0),vec3<f32>(1.0));
    if settings.display.y>0.0 {
        let n=settings.display.z;
        let coords=(x*(n-1.0)+0.5)/n;
        x=mix(x,textureSampleLevel(lut,linear_sampler,coords,0.0).rgb,settings.display.y);
    }
    let edge=smoothstep(0.15,1.8,dot(radial,radial));
    x*=1.0-settings.grade.z*edge;
    if settings.grade.w>0.0 {
        let signal=sqrt(clamp(dot(x,vec3<f32>(0.2126,0.7152,0.0722))*4.0,0.0,1.0));
        x+=(hash(vec2<u32>(p.xy),u32(settings.bloom_motion.w))-0.5)*settings.grade.w*signal;
    }
    return vec4<f32>(clamp(x,vec3<f32>(0.0),vec3<f32>(1.0)),1.0);
}
