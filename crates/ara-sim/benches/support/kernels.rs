//! SIMD across four entities, preserving S8's arithmetic and AoS component layout.
use super::baseline::{Position, Rotation, Velocity};
use crate::model::{self, DT};
use glam::{Mat4, Vec4};

/// W1: transpose four quaternions/velocities into SIMD lanes, then transpose back.
pub fn transforms(p: &mut [Position], v: &[Velocity], q: &mut [Rotation]) {
    let (pp, pt) = p.as_chunks_mut::<4>();
    let (vv, vt) = v.as_chunks::<4>();
    let (qq, qt) = q.as_chunks_mut::<4>();
    for ((p, v), q) in pp.iter_mut().zip(vv).zip(qq) {
        for i in 0..4 {
            p[i].0 += v[i].0 * DT;
        }
        let a = Mat4::from_cols(v[0].0 * 0.1, v[1].0 * 0.1, v[2].0 * 0.1, v[3].0 * 0.1).transpose();
        let r = Mat4::from_cols(q[0].0, q[1].0, q[2].0, q[3].0).transpose();
        let dx = a.x_axis * r.w_axis + a.y_axis * r.z_axis - a.z_axis * r.y_axis;
        let dy = a.y_axis * r.w_axis + a.z_axis * r.x_axis - a.x_axis * r.z_axis;
        let dz = a.z_axis * r.w_axis + a.x_axis * r.y_axis - a.y_axis * r.x_axis;
        let dw = -((a.x_axis * r.x_axis + a.y_axis * r.y_axis) + a.z_axis * r.z_axis);
        let x = r.x_axis + dx * (0.5 * DT);
        let y = r.y_axis + dy * (0.5 * DT);
        let z = r.z_axis + dz * (0.5 * DT);
        let w = r.w_axis + dw * (0.5 * DT);
        let norm = ((x * x + y * y) + z * z) + w * w;
        let inv = Vec4::ONE / Vec4::from_array(norm.to_array().map(f32::sqrt));
        let out = Mat4::from_cols(x * inv, y * inv, z * inv, w * inv).transpose();
        [q[0].0, q[1].0, q[2].0, q[3].0] = [out.x_axis, out.y_axis, out.z_axis, out.w_axis];
    }
    for ((p, v), q) in pt.iter_mut().zip(vt).zip(qt) {
        model::transform(&mut p.0, v.0, &mut q.0);
    }
}

/// W3: four particles share a branchless bounce mask, without changing math order.
pub fn particles(p: &mut [Position], v: &mut [Velocity]) {
    let (pp, pt) = p.as_chunks_mut::<4>();
    let (vv, vt) = v.as_chunks_mut::<4>();
    for (p, v) in pp.iter_mut().zip(vv) {
        let mut pos = Mat4::from_cols(p[0].0, p[1].0, p[2].0, p[3].0).transpose();
        let mut vel = Mat4::from_cols(v[0].0, v[1].0, v[2].0, v[3].0).transpose();
        vel.x_axis = (vel.x_axis + Vec4::ZERO) * 0.999;
        vel.y_axis = (vel.y_axis + Vec4::splat(-9.81) * DT) * 0.999;
        vel.z_axis = (vel.z_axis + Vec4::ZERO) * 0.999;
        vel.w_axis = (vel.w_axis + Vec4::ZERO) * 0.999;
        pos.x_axis += vel.x_axis * DT;
        pos.y_axis += vel.y_axis * DT;
        pos.z_axis += vel.z_axis * DT;
        pos.w_axis += vel.w_axis * DT;
        let bounce = pos.y_axis.cmplt(Vec4::ZERO);
        pos.y_axis = Vec4::select(bounce, -pos.y_axis, pos.y_axis);
        vel.y_axis = Vec4::select(bounce, vel.y_axis.abs() * 0.8, vel.y_axis);
        let p_out = pos.transpose();
        let v_out = vel.transpose();
        [p[0].0, p[1].0, p[2].0, p[3].0] = [p_out.x_axis, p_out.y_axis, p_out.z_axis, p_out.w_axis];
        [v[0].0, v[1].0, v[2].0, v[3].0] = [v_out.x_axis, v_out.y_axis, v_out.z_axis, v_out.w_axis];
    }
    for (p, v) in pt.iter_mut().zip(vt) {
        model::particle(&mut p.0, &mut v.0);
    }
}
