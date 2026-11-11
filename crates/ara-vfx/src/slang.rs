use crate::{Effect, Module, Stacks, Trigger, error};
use ara_diag::Diagnostic;
use std::fmt::Write;

fn f(x: f32) -> String {
    let s = ara_json::format_f64(f64::from(x));
    if s.contains(['.', 'e', 'E']) {
        s
    } else {
        format!("{s}.0")
    }
}

fn v(xs: &[f32]) -> String {
    format!(
        "float{}({})",
        xs.len(),
        xs.iter().map(|x| f(*x)).collect::<Vec<_>>().join(",")
    )
}
fn line(out: &mut String, args: std::fmt::Arguments<'_>) {
    out.write_fmt(args)
        .expect("writing into a String cannot fail");
    out.push('\n');
}

/// `target = curve(t)` in Slang: `t` brought into the keyed span by the curve's
/// extrapolation, then each segment's polynomial (`Curve::polynomial`), as the CPU's
/// `Curve::evaluate` computes it.
fn curve_lines(out: &mut String, target: &str, curve: &ara_curve::Curve) {
    use ara_curve::Extrapolation;
    let (Some(first), Some(last)) = (curve.keys.first(), curve.keys.last()) else {
        line(out, format_args!("{target}=0.0;"));
        return;
    };
    // Straight, clamped segments lower as they always have (generated shaders unchanged).
    if curve
        .keys
        .iter()
        .all(|k| k.interpolation == ara_curve::Interpolation::Linear)
        && (curve.before, curve.after) == (Extrapolation::Clamp, Extrapolation::Clamp)
    {
        line(out, format_args!("{target}={};", f(first.value)));
        for pair in curve.keys.windows(2) {
            line(
                out,
                format_args!(
                    "{target}=lerp({target},{},saturate((t-{})/{}));",
                    f(pair[1].value),
                    f(pair[0].time),
                    f(pair[1].time - pair[0].time)
                ),
            );
        }
        return;
    }
    let (start, length) = (first.time, last.time - first.time);
    line(out, format_args!("{{float ct=t;"));
    for (mode, test) in [
        (curve.before, format!("ct<{}", f(start))),
        (curve.after, format!("ct>{}", f(last.time))),
    ] {
        if length <= 0.0 {
            break;
        }
        let (s, l) = (f(start), f(length));
        match mode {
            Extrapolation::Clamp => {}
            Extrapolation::Loop => line(
                out,
                format_args!("if({test}){{float x=ct-{s};ct={s}+x-{l}*floor(x/{l});}}"),
            ),
            Extrapolation::PingPong => line(
                out,
                format_args!(
                    "if({test}){{float x=ct-{s};float ph=x-2.0*{l}*floor(x/(2.0*{l}));ct={s}+(ph>{l}?2.0*{l}-ph:ph);}}"
                ),
            ),
        }
    }
    line(
        out,
        format_args!("if(ct<{}){{{target}={};}}", f(start), f(first.value)),
    );
    for (i, pair) in curve.keys.windows(2).enumerate() {
        let [c0, c1, c2, c3] = curve.polynomial(i);
        line(
            out,
            format_args!(
                "else if(ct<{}){{float u=(ct-{})/{};{target}={}+u*({}+u*({}+u*{}));}}",
                f(pair[1].time),
                f(pair[0].time),
                f(pair[1].time - pair[0].time),
                f(c0),
                f(c1),
                f(c2),
                f(c3)
            ),
        );
    }
    line(out, format_args!("else{{{target}={};}}}}", f(last.value)));
}

/// Generate specialized module functions for the GPU runtime's Particle contract.
/// This is pure source generation; cook it in an external background worker (D20).
pub fn generate(effect: &Effect) -> Result<String, Diagnostic> {
    let effect = effect.resolve(&Default::default())?;
    let mut out = format!("// VFX content {}\n", effect.content_hash()?);
    out.push_str("void initializeParticle(uint emitter, inout Particle p) { uint rng=p.seed; switch(emitter) {\n");
    for (i, e) in effect.emitters.iter().enumerate() {
        line(&mut out, format_args!("case {i}u: {{"));
        for item in &e.stacks.initialize {
            out.push_str("{\n");
            match &item.module {
                Module::Lifetime {
                    seconds,
                    size,
                    color,
                } => line(
                    &mut out,
                    format_args!(
                        "p.life={}; p.size={}; p.color={};",
                        f(*seconds),
                        f(*size),
                        v(color)
                    ),
                ),
                Module::Point { position } => {
                    line(&mut out, format_args!("p.position={};", v(position)))
                }
                Module::Sphere { center, radius } => line(
                    &mut out,
                    format_args!(
                        "float3 r=random3(rng); float z=2*r.x-1; float a=6.28318530718*r.y; float xy=sqrt(max(0.,1-z*z)); p.position={}+{}*pow(r.z,1./3.)*float3(xy*cos(a),z,xy*sin(a));",
                        v(center),
                        f(*radius)
                    ),
                ),
                Module::Box { center, extent } => line(
                    &mut out,
                    format_args!("p.position={}+(2*random3(rng)-1)*{};", v(center), v(extent)),
                ),
                Module::Cone {
                    origin,
                    height,
                    radius,
                } => line(
                    &mut out,
                    format_args!(
                        "float3 r=random3(rng); float h=pow(r.x,1./3.);float rad=sqrt(r.y)*h*{};float a=6.28318530718*r.z;p.position={}+float3(rad*cos(a),h*{},rad*sin(a));",
                        f(*radius),
                        v(origin),
                        f(*height)
                    ),
                ),
                Module::Velocity { value, spread } => line(
                    &mut out,
                    format_args!("p.velocity={}+(2*random3(rng)-1)*{};", v(value), v(spread)),
                ),
                Module::MeshSurface { triangles } => {
                    let mut total = 0.;
                    let mut cdf = Vec::new();
                    for t in triangles {
                        total += (glam::Vec3::from(t[1]) - glam::Vec3::from(t[0]))
                            .cross(glam::Vec3::from(t[2]) - glam::Vec3::from(t[0]))
                            .length();
                        cdf.push(total);
                    }
                    line(
                        &mut out,
                        format_args!(
                            "float pick=random(rng)*{};float u=sqrt(random(rng));float b=random(rng);",
                            f(total)
                        ),
                    );
                    for (index, (t, area)) in triangles.iter().zip(cdf).enumerate() {
                        line(
                            &mut out,
                            format_args!(
                                "{} (pick<={}) {{p.position=(1-u)*{}+u*(1-b)*{}+u*b*{};}}",
                                if index == 0 { "if" } else { "else if" },
                                f(area),
                                v(&t[0]),
                                v(&t[1]),
                                v(&t[2])
                            ),
                        );
                    }
                }
                _ => return Err(error("illegal initialize module")),
            }
            out.push_str("}\n");
        }
        out.push_str("break;}\n");
    }
    out.push_str("}}\nvoid updateParticle(uint emitter,inout Particle p,float3 previous,float dt,inout uint events) {float t=saturate(p.age/p.life);switch(emitter){\n");
    for (i, e) in effect.emitters.iter().enumerate() {
        line(&mut out, format_args!("case {i}u: {{"));
        for item in &e.stacks.update {
            out.push_str("{\n");
            match &item.module {
                Module::Gravity { acceleration } => line(
                    &mut out,
                    format_args!("p.velocity+={}*dt;", v(acceleration)),
                ),
                Module::Drag { coefficient } => line(
                    &mut out,
                    format_args!("p.velocity/=1.+{}*dt;", f(*coefficient)),
                ),
                Module::CurlNoise {
                    frequency,
                    strength,
                } => line(
                    &mut out,
                    format_args!(
                        "float3 q=p.position*{}+float(p.birth)*0.013;float3 c=cos(q);p.velocity+={}*dt*float3(c.y-c.z,c.z-c.x,c.x-c.y);",
                        f(*frequency),
                        f(*strength)
                    ),
                ),
                Module::Vortex {
                    center,
                    axis,
                    strength,
                } => line(
                    &mut out,
                    format_args!(
                        "p.velocity+=cross(normalize({}),p.position-{})*{}*dt;",
                        v(axis),
                        v(center),
                        f(*strength)
                    ),
                ),
                Module::Attractor {
                    position,
                    strength,
                    softening,
                } => line(
                    &mut out,
                    format_args!(
                        "float3 d={}-p.position;p.velocity+=d*({}*dt/pow(dot(d,d)+{},1.5));",
                        v(position),
                        f(*strength),
                        f(*softening)
                    ),
                ),
                Module::ColorOverLife { keys } => {
                    line(&mut out, format_args!("p.color={};", v(&keys[0].value)));
                    for pair in keys.windows(2) {
                        line(
                            &mut out,
                            format_args!(
                                "p.color=lerp(p.color,{},saturate((t-{})/{}));",
                                v(&pair[1].value),
                                f(pair[0].t),
                                f(pair[1].t - pair[0].t)
                            ),
                        );
                    }
                }
                Module::SizeOverLife { curve } => curve_lines(&mut out, "p.size", curve),
                Module::PlaneCollision {
                    normal,
                    offset,
                    restitution,
                } => line(
                    &mut out,
                    format_args!(
                        "float3 n=normalize({});float distance=dot(p.position,n)-{};if(distance<0){{p.position-=distance*n;float vn=dot(p.velocity,n);if(vn<0){{p.velocity-=(1.+{})*vn*n;events|=1u;}}}}",
                        v(normal),
                        f(*offset),
                        f(*restitution)
                    ),
                ),
                Module::DepthCollision {
                    restitution,
                    thickness,
                } => line(
                    &mut out,
                    format_args!(
                        "collideDepth(p,previous,{}, {},events);",
                        f(*restitution),
                        f(*thickness)
                    ),
                ),
                _ => return Err(error("illegal update module")),
            }
            out.push_str("}\n");
        }
        out.push_str("break;}\n");
    }
    out.push_str("}}\nuint2 eventTarget(uint source,uint trigger) {switch(source){\n");
    for (i, e) in effect.emitters.iter().enumerate() {
        line(&mut out, format_args!("case {i}u: {{"));
        for m in &e.stacks.render {
            if let Module::EventSpawn {
                trigger,
                target,
                count,
            } = &m.module
            {
                let dest = effect
                    .emitters
                    .iter()
                    .position(|e| &e.name == target)
                    .ok_or_else(|| error("missing event target"))?;
                line(
                    &mut out,
                    format_args!(
                        "if(trigger=={}u)return uint2({dest}u,{count}u);",
                        if *trigger == Trigger::Collision { 1 } else { 2 }
                    ),
                );
            }
        }
        out.push_str("break;}\n");
    }
    out.push_str("}return uint2(0xffffffffu,0);}\nfloat3 meshVertex(uint emitter,uint vertex){switch(emitter){\n");
    for (i, e) in effect.emitters.iter().enumerate() {
        for m in &e.stacks.render {
            if let Module::Mesh { triangles, .. } = &m.module {
                line(&mut out, format_args!("case {i}u: switch(vertex){{"));
                for (j, vert) in triangles.iter().flatten().enumerate() {
                    line(&mut out, format_args!("case {j}u:return {};", v(vert)));
                }
                out.push_str("}break;\n");
            }
        }
    }
    out.push_str("}return float3(0);}\n");
    // Stage identities and node names remain in the source for agent diagnostics.
    for e in &effect.emitters {
        for s in Stacks::STAGES {
            for m in e.stacks.get(s) {
                line(
                    &mut out,
                    format_args!("// {} / {s:?} / {}: {}", e.name, m.name, m.module.kind()),
                );
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn scalar_literals_preserve_integer_magnitude_and_sign() {
        for value in [0., 1., -1., 2., 10., 100., 0.01, -9.81] {
            let literal = super::f(value);
            assert_eq!(literal.parse::<f32>().unwrap(), value);
        }
    }
}
