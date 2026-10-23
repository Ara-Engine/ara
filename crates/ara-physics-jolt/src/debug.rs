use crate::JoltBackend;
use ara_physics::*;

impl JoltBackend {
    pub(crate) fn draw(&self, out: &mut Vec<DebugPrimitive>) -> Result<()> {
        out.clear();
        for (&body, meta) in &self.bodies {
            let state = self.extract_body(body)?;
            out.push(DebugPrimitive::Shape {
                body,
                shape: meta.shape,
                pose: state.pose,
                sleeping: !state.active,
            });
        }
        for (sensor, contact) in self.contacts.values() {
            out.push(DebugPrimitive::Contact {
                point: contact.point,
                normal: contact.normal,
                sensor: *sensor,
            });
        }
        for contact in self.character_contacts.values() {
            out.push(DebugPrimitive::Contact {
                point: contact.point,
                normal: contact.normal,
                sensor: contact.sensor,
            });
        }
        for (&id, c) in &self.characters {
            let c = &c.controller;
            let state = self.extract_character(id)?;
            out.push(DebugPrimitive::Character {
                feet: state.position,
                height: if state.crouched {
                    c.crouch_height
                } else {
                    c.height
                },
                radius: c.radius,
                grounded: state.grounded,
            });
        }
        for (&id, meta) in &self.vehicles {
            let state = self.extract_vehicle(id)?;
            for center in state.wheel_positions {
                out.push(DebugPrimitive::Wheel {
                    center,
                    radius: meta.desc.vehicle.wheel_radius,
                });
            }
        }
        for d in self.joints.values() {
            out.push(DebugPrimitive::Joint {
                anchor_a: d.joint.anchor_a,
                anchor_b: d.joint.anchor_b,
                axis: d.joint.axis,
            });
        }
        Ok(())
    }
}
