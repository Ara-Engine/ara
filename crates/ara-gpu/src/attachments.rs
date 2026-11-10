//! Load and store operations of rendering-scope attachments, from the contents a
//! list leaves dead. Both backends lower [`Command::BeginRendering`] through
//! [`ops`], so tile-based GPUs skip reading or writing back contents nobody uses:
//!
//! - a discard barrier (`after: Access::Undefined`, emitted by the render graph after
//!   a resource's last use) right after a scope stores none of that attachment, and
//!   a discarded colour attachment loads nothing either (only blending reads it);
//! - an attachment whose contents are undefined (a barrier from `Undefined`, or a
//!   discard earlier in the list) loads nothing when the scope does not clear it;
//! - a transient attachment (`TRANSIENT_ATTACHMENT` usage: Metal memoryless, Vulkan
//!   lazily allocated memory) keeps contents only within one scope.
use crate::commands::Command;
use crate::{Access, GpuError, Image, Resource, Result, vk};

/// What a scope does with an attachment's earlier contents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Load {
    /// Fill with the attachment's clear value.
    Clear,
    /// Read the earlier contents.
    Keep,
    /// Start from undefined contents.
    DontCare,
}

/// Load and store operations of one attachment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ops {
    pub load: Load,
    /// Write the contents back for later use.
    pub store: bool,
}

/// Operations for each attachment (colours in order, then depth) of each rendering
/// scope in `commands`, in scope order. `transient` tells transient images apart.
pub(crate) fn ops(commands: &[Command], transient: impl Fn(Image) -> bool) -> Vec<Vec<Ops>> {
    use crate::rendering::RenderTarget;
    let mut states = Vec::<(RenderTarget, bool)>::new();
    for c in commands {
        if let Some(colors) = c.color_attachments() {
            for a in colors.chain(c.depth_attachment()) {
                if !states.iter().any(|(t, _)| *t == a.target) {
                    states.push((a.target, false));
                }
            }
        }
        for target in c.resolve_targets().flatten() {
            if !states.iter().any(|(t, _)| *t == target) {
                states.push((target, false));
            }
        }
    }
    let mut scopes = Vec::new();
    for (index, command) in commands.iter().enumerate() {
        for b in command.barriers() {
            for (target, undefined) in &mut states {
                if covers(b, *target) {
                    *undefined = b.barrier.before == Access::Undefined
                        || b.barrier.after == Access::Undefined;
                }
            }
        }
        if let Some(colors) = command.color_attachments() {
            let count = colors.len();
            let discarded = discarded_after(&commands[index + 1..]);
            let ops = colors
                .chain(command.depth_attachment())
                .enumerate()
                .map(|(i, a)| {
                    let transient = transient(a.target.image);
                    let dead_after = transient || discarded.iter().any(|b| covers(*b, a.target));
                    let undefined = states.iter().any(|(t, u)| *t == a.target && *u);
                    let dead_before = transient || undefined || (dead_after && i < count);
                    Ops {
                        load: match (a.clear, dead_before) {
                            (Some(_), _) => Load::Clear,
                            (None, true) => Load::DontCare,
                            (None, false) => Load::Keep,
                        },
                        store: !dead_after,
                    }
                })
                .collect();
            scopes.push(ops);
            for a in command
                .color_attachments()
                .into_iter()
                .flatten()
                .chain(command.depth_attachment())
            {
                for (target, undefined) in &mut states {
                    if *target == a.target {
                        *undefined = false;
                    }
                }
            }
            for resolved in command.resolve_targets().flatten() {
                for (target, undefined) in &mut states {
                    if *target == resolved {
                        *undefined = false;
                    }
                }
            }
        }
    }
    scopes
}
fn covers(b: crate::sync::ResourceBarrier, t: crate::rendering::RenderTarget) -> bool {
    if b.barrier.resource != Resource::Image(t.image) {
        return false;
    }
    match b.range {
        crate::sync::BarrierRange::Whole => true,
        crate::sync::BarrierRange::Image {
            base_mip,
            mip_count,
            base_layer,
            layer_count,
        } => {
            (base_mip..base_mip.saturating_add(mip_count)).contains(&t.mip)
                && (base_layer..base_layer.saturating_add(layer_count)).contains(&t.layer)
        }
        _ => false,
    }
}
/// Discard ranges immediately following this scope, before the next GPU operation.
fn discarded_after(rest: &[Command]) -> Vec<crate::sync::ResourceBarrier> {
    rest.iter()
        .skip_while(|c| !matches!(c, Command::EndRendering))
        .skip(1)
        .take_while(|c| c.is_barrier() || matches!(c, Command::BeginPass(_) | Command::EndPass))
        .flat_map(|c| c.barriers())
        .filter(|b| b.barrier.after == Access::Undefined)
        .collect()
}

/// Transient images are attachments only: Metal memoryless and Vulkan lazily
/// allocated storage cannot be sampled, written by shaders or copied.
pub(crate) fn validate_usage(usage: vk::ImageUsageFlags) -> Result<()> {
    let attachment = vk::ImageUsageFlags::COLOR_ATTACHMENT
        | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
        | vk::ImageUsageFlags::INPUT_ATTACHMENT
        | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT;
    if usage.contains(vk::ImageUsageFlags::TRANSIENT_ATTACHMENT) && !attachment.contains(usage) {
        return Err(GpuError::Contract(
            "Transient attachments cannot be sampled, storage or copy images".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Barrier, commands::Attachment};

    #[test]
    fn a_layer_discard_preserves_sibling_contents() {
        use crate::{
            rendering::{RenderAttachment, RenderTarget},
            sync::{BarrierRange, ResourceBarrier},
        };
        let [image] = images();
        let target = |layer| RenderAttachment {
            target: RenderTarget {
                image,
                mip: 1,
                layer,
            },
            clear: None,
        };
        let commands = [
            Command::BeginRenderingSubresources {
                colors: vec![target(0), target(1)],
                depth: None,
                resolves: vec![],
                size: [4, 4],
            },
            Command::EndRendering,
            Command::Barriers(vec![ResourceBarrier {
                barrier: Barrier {
                    resource: Resource::Image(image),
                    before: Access::Color,
                    after: Access::Undefined,
                    handoff: None,
                },
                range: BarrierRange::Image {
                    base_mip: 1,
                    mip_count: 1,
                    base_layer: 1,
                    layer_count: 1,
                },
            }]),
            Command::BeginRenderingSubresources {
                colors: vec![target(0), target(1)],
                depth: None,
                resolves: vec![],
                size: [4, 4],
            },
            Command::EndRendering,
        ];
        let result = ops(&commands, |_| false);
        assert!(result[0][0].store);
        assert!(!result[0][1].store);
        assert_eq!(result[1][0].load, Load::Keep);
        assert_eq!(result[1][1].load, Load::DontCare);
    }
    fn images<const N: usize>() -> [Image; N] {
        let mut arena = crate::arena::Arena::<crate::ImageKind, ()>::default();
        std::array::from_fn(|_| arena.insert(()).unwrap())
    }
    fn barrier(image: Image, before: Access, after: Access) -> Command {
        Command::Barrier(Barrier {
            resource: Resource::Image(image),
            before,
            after,
            handoff: None,
        })
    }
    fn scope(colors: &[Image], depth: Option<Image>) -> Command {
        let attachment = |image| Attachment { image, clear: None };
        Command::BeginRendering {
            colors: colors.iter().copied().map(attachment).collect(),
            depth: depth.map(attachment),
            size: [8, 8],
        }
    }
    const KEEP: Ops = Ops {
        load: Load::Keep,
        store: true,
    };

    #[test]
    fn a_discard_after_the_scope_skips_the_store_and_a_colour_load() {
        let [kept, dead, depth] = images();
        let commands = [
            scope(&[kept, dead], Some(depth)),
            Command::EndRendering,
            Command::EndPass,
            barrier(dead, Access::Color, Access::Undefined),
            barrier(depth, Access::Depth, Access::Undefined),
        ];
        let dead_colour = Ops {
            load: Load::DontCare,
            store: false,
        };
        // The depth test still reads the depth contents.
        let dead_depth = Ops {
            load: Load::Keep,
            store: false,
        };
        assert_eq!(
            ops(&commands, |_| false),
            [vec![KEEP, dead_colour, dead_depth]]
        );
    }

    #[test]
    fn work_between_the_scope_and_a_discard_keeps_the_contents() {
        let [color] = images();
        let commands = [
            scope(&[color], None),
            Command::EndRendering,
            Command::Dispatch([1, 1, 1]),
            barrier(color, Access::Color, Access::Undefined),
        ];
        assert_eq!(ops(&commands, |_| false), [vec![KEEP]]);
    }

    #[test]
    fn undefined_contents_load_nothing_until_a_scope_writes_them() {
        let [color] = images();
        let commands = [
            barrier(color, Access::Undefined, Access::Color),
            scope(&[color], None),
            Command::EndRendering,
            barrier(color, Access::Color, Access::Color),
            scope(&[color], None),
            Command::EndRendering,
        ];
        let first = Ops {
            load: Load::DontCare,
            store: true,
        };
        assert_eq!(ops(&commands, |_| false), [vec![first], vec![KEEP]]);
    }

    #[test]
    fn transient_attachments_keep_contents_within_one_scope() {
        let [color, depth] = images();
        let mut commands = vec![scope(&[color], Some(depth)), Command::EndRendering];
        if let Command::BeginRendering { colors, .. } = &mut commands[0] {
            colors[0].clear = Some([0.; 4]);
        }
        let dead = |load| Ops { load, store: false };
        assert_eq!(
            ops(&commands, |_| true),
            [vec![dead(Load::Clear), dead(Load::DontCare)]]
        );
    }

    #[test]
    fn transient_images_are_attachments_only() {
        let transient = vk::ImageUsageFlags::TRANSIENT_ATTACHMENT;
        assert!(validate_usage(transient | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT).is_ok());
        assert!(
            validate_usage(
                transient | vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED
            )
            .is_err()
        );
        assert!(validate_usage(vk::ImageUsageFlags::SAMPLED).is_ok());
    }
}
