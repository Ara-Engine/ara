//! Audited version-one full-state export; no instrumentation enters timed steps.
use super::model::*;
use bevy_ecs::prelude::*;
use std::{error::Error, io::Write, path::Path};

pub fn write(
    world: &mut World,
    case: Case,
    frame: Frame,
    steps: u32,
    path: &Path,
) -> Result<(u64, String, serde_json::Value), Box<dyn Error>> {
    let mut query = world.query::<(Entity, &Id, &Value)>();
    let mut entities = vec![None; frame.count as usize];
    for (entity, id, _) in query.iter(world) {
        let slot = entities.get_mut(id.0 as usize).ok_or("out-of-range ID")?;
        if slot.replace(entity).is_some() {
            return Err("duplicate ID".into());
        }
    }
    let mut file = std::io::BufWriter::new(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?,
    );
    let mut hash = blake3::Hasher::new();
    let mut bytes = 0_u64;
    let mut append = |data: &[u8]| -> std::io::Result<()> {
        file.write_all(data)?;
        hash.update(data);
        bytes += data.len() as u64;
        Ok(())
    };
    append(b"ARAECSF1")?;
    for word in [1, case.id(), frame.count, steps] {
        append(&word.to_le_bytes())?;
    }
    let mut capacities = std::collections::BTreeMap::<usize, u32>::new();
    let mut spills = 0_u32;
    let mut live_words = 0_usize;
    for (id, entity) in entities.into_iter().enumerate() {
        let entity = entity.ok_or("missing ID")?;
        if world.get::<Epoch>(entity).is_some() != (case == Case::Dirty)
            || world.get::<StageA>(entity).is_some() != (case == Case::Dag)
            || world.get::<StageB>(entity).is_some() != (case == Case::Dag)
            || world.get::<StageC>(entity).is_some() != (case == Case::Dag)
            || world.get::<Words>(entity).is_some() != matches!(case, Case::Fixed | Case::Resize)
        {
            return Err("component presence differs".into());
        }
        let value = world.get::<Value>(entity).ok_or("missing Value")?;
        let epoch = world.get::<Epoch>(entity).map_or(0, |v| v.0);
        let tags = if world.get::<TagD>(entity).is_some() {
            8
        } else {
            0
        };
        let mut expected_updates = steps;
        let mut expected_epoch = 0;
        if case == Case::Dirty {
            expected_updates = 0;
            for tick in 0..steps {
                if selected(id as u32, Frame { tick, ..frame }) {
                    expected_updates += 1;
                    expected_epoch = tick + 1;
                }
            }
        } else if case == Case::Churn {
            expected_updates = if id as u32 % 100 < frame.percent {
                steps.div_ceil(2)
            } else {
                0
            };
        }
        let expected_tags = if case == Case::Churn
            && steps > 0
            && (steps - 1).is_multiple_of(2)
            && id as u32 % 100 < frame.percent
        {
            8
        } else {
            0
        };
        if tags != expected_tags || epoch != expected_epoch || value.updates != expected_updates {
            return Err("tag/epoch/update audit failed".into());
        }
        let words = world.get::<Words>(entity);
        let expected_length = match case {
            Case::Fixed => frame.length,
            Case::Resize
                if steps > 0
                    && (steps - 1).is_multiple_of(2)
                    && id as u32 % 100 < frame.percent =>
            {
                64
            }
            Case::Resize => 4,
            _ => 0,
        };
        if words.map_or(0, |v| v.0.len()) != expected_length {
            return Err("live buffer length differs".into());
        }
        if let Some(words) = words {
            if case == Case::Resize && steps >= 2 && id as u32 % 100 < frame.percent {
                let short_trim = steps.is_multiple_of(2) && frame.trim;
                if short_trim {
                    if words.0.capacity() != 4 || words.0.spilled() {
                        return Err("trimmed short buffer did not return inline".into());
                    }
                } else if words.0.capacity() < 64 || !words.0.spilled() {
                    return Err("grown/reused buffer lost external capacity".into());
                }
            }
            *capacities.entry(words.0.capacity()).or_default() += 1;
            spills += u32::from(words.0.spilled());
            live_words += words.0.len();
        }
        let a = world.get::<StageA>(entity).map_or(0, |v| v.0);
        let b = world.get::<StageB>(entity).map_or(0, |v| v.0);
        let c = world.get::<StageC>(entity).map_or(0, |v| v.0);
        if case != Case::Dag && (a != 0 || b != 0 || c != 0) {
            return Err("inactive stage columns nonzero".into());
        }
        for word in [
            id as u32,
            tags,
            value.value,
            epoch,
            a,
            b,
            c,
            value.updates,
            expected_length as u32,
        ] {
            append(&word.to_le_bytes())?;
        }
        if let Some(words) = words {
            for word in &words.0 {
                append(&word.to_le_bytes())?;
            }
        }
    }
    file.flush()?;
    Ok((
        bytes,
        hash.finalize().to_hex().to_string(),
        serde_json::json!({
            "smallvec_inline_words": 4, "buffer_component_bytes": std::mem::size_of::<Words>(),
            "buffer_component_alignment": std::mem::align_of::<Words>(), "capacity_histogram": capacities,
            "spilled_components": spills, "live_words": live_words,
            "capacity_observation": "post-run only; not an allocation counter",
            "resize_storage_policy_checked": case == Case::Resize && steps >= 2,
            "components": component_layouts(case)
        }),
    ))
}

fn component<T>(name: &str) -> serde_json::Value {
    serde_json::json!({"name": name, "bytes": std::mem::size_of::<T>(), "alignment": std::mem::align_of::<T>()})
}

fn component_layouts(case: Case) -> Vec<serde_json::Value> {
    let mut layouts = vec![component::<Id>("Id"), component::<Value>("Value")];
    match case {
        Case::Dirty => layouts.push(component::<Epoch>("Epoch")),
        Case::Churn => layouts.push(component::<TagD>("TagD")),
        Case::Dag => layouts.extend([
            component::<StageA>("StageA"),
            component::<StageB>("StageB"),
            component::<StageC>("StageC"),
        ]),
        Case::Fixed | Case::Resize => layouts.push(component::<Words>("Words")),
    }
    layouts
}
