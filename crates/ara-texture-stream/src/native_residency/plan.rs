use super::*;

pub(super) fn select(entries: &[Entry], budget: u64, out: &mut Vec<u32>) -> Result<(u64, u64)> {
    out.clear();
    let mut total = 0u64;
    for entry in entries {
        out.push(entry.desired);
        total = total
            .checked_add(bytes(entry, entry.desired)?)
            .ok_or_else(|| anyhow::anyhow!("native texture payload overflow"))?;
    }
    let requested = total;
    while total > budget {
        let mut best = None;
        for (index, entry) in entries.iter().enumerate() {
            let mip = out[index];
            if mip + 1 == entry.source.mip_count() {
                continue;
            }
            let saving = bytes(entry, mip)? - bytes(entry, mip + 1)?;
            let score = saving as f64 / (1.0 + f64::from(entry.priority));
            if best.is_none_or(|(_, _, previous)| score > previous) {
                best = Some((index, saving, score));
            }
        }
        let Some((index, saving, _)) = best else {
            anyhow::bail!("native texture budget cannot hold every minimum mip tail");
        };
        out[index] += 1;
        total -= saving;
    }
    Ok((total, requested))
}

pub(super) fn bytes(entry: &Entry, first: u32) -> Result<u64> {
    entry
        .source
        .resident_bytes(first)
        .ok_or_else(|| anyhow::anyhow!("native resident mip is out of range"))
}
