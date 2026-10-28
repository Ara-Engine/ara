//! Authority identity checks are independent of the UI controls being exercised.
use super::*;
use ara_runtime_render::probes::ProbeReceipt;
#[cfg(test)]
mod tests;
pub(super) fn probe(editor: &Editor, entity: &str) -> ara_ui::Result<Value> {
    fixture::records(editor)?
        .get(entity)
        .and_then(|record| record.get("ReflectionProbe"))
        .cloned()
        .ok_or_else(|| "Native ReflectionProbe component missing".into())
}
pub(super) fn revision(before: &Value, after: &Value, delta: u64) -> ara_ui::Result<()> {
    if before["revision"]
        .as_u64()
        .is_none_or(|revision| after["revision"] != revision + delta)
    {
        return Err(format!(
            "Expected exactly{delta} authoring revisions; before {}, after {}",
            before["revision"], after["revision"]
        )
        .into());
    }
    Ok(())
}
pub(super) fn one_transaction(before: &Value, after: &Value) -> ara_ui::Result<()> {
    revision(before, after, 1)?;
    if before["records"] == after["records"] {
        return Err("Native mutation produced no actual authored change".into());
    }
    Ok(())
}
pub(super) fn unchanged(before: &Value, after: &Value) -> ara_ui::Result<()> {
    revision(before, after, 0)?;
    if before["records"] != after["records"] {
        return Err("Derived work/navigation changed authored records".into());
    }
    Ok(())
}
fn hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
pub(super) fn published_receipt(row: &ProbeRow, receipt: &ProbeReceipt) -> ara_ui::Result<Value> {
    if receipt.doc != SCENE
        || receipt.entity != row.entity
        || receipt.faces != 6
        || receipt.resolution != 128
        || !receipt.published
        || receipt.generation != row.generation
        || receipt.source_revision != row.scene_revision
        || receipt.source_hash != row.source_hash
        || row.stale
        || !hash(&receipt.source_hash)
        || !hash(&receipt.capture_hash)
    {
        return Err(format!("Published capture receipt is incomplete or does not match its current source: {receipt:?}").into());
    }
    let path = Path::new(&receipt.cache_path);
    if !receipt.cache_path.starts_with(".ara/cache/")
        || receipt.cache_path.contains('\\')
        || path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err("Probe receipt cache path leaves the private project cache".into());
    }
    // Done attests publication plus the image hash. Cache IO completes afterward;
    // the native harness observes its actual file on a separate bounded worker.
    Ok(
        json!({"path":receipt.cache_path,"publishedImageHashed":true,"cachePublication":"asynchronous"}),
    )
}
/// Pending adapters return null. Missing completed-image provenance or actual
/// overdraw data never becomes a navigation-only pass.
pub(super) fn debug_view(value: &Value, mode: &str) -> ara_ui::Result<Value> {
    if value.is_null()
        || value["mode"] != mode
        || value["completed"] != true
        || value["current"] != true
    {
        return Ok(Value::Null);
    }
    if value["observationSerial"]
        .as_u64()
        .is_none_or(|generation| generation == 0)
    {
        return Err("Debug view lacks a completed-image observation serial".into());
    }
    if mode == "overdraw" {
        let stats = &value["overdraw"];
        let pixels = stats["pixels"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("Completed overdraw metadata has no measured pixels")?;
        let max = stats["max"]
            .as_u64()
            .filter(|n| *n >= 1 && *n <= u32::MAX as u64)
            .ok_or("Completed overdraw fixture has no fragment counts")?;
        let bands = stats["bands"]
            .as_array()
            .filter(|bands| bands.len() == 6)
            .ok_or("Overdraw metadata lacks its six measured bands")?;
        let mut sum = 0u64;
        let mut highest = None;
        for (index, band) in bands.iter().enumerate() {
            let count = band
                .as_u64()
                .ok_or("Overdraw band is not an unsigned pixel count")?;
            sum = sum
                .checked_add(count)
                .ok_or("Overdraw band counts overflow")?;
            if count > 0 {
                highest = Some(index);
            }
        }
        let max_band = match max {
            1 => 1,
            2..=3 => 2,
            4..=7 => 3,
            8..=15 => 4,
            _ => 5,
        };
        if sum != pixels || highest != Some(max_band) {
            return Err(
                "Actual overdraw pixel bands do not cover the image or match its maximum".into(),
            );
        }
    }
    Ok(value.clone())
}
