//! Pure AGX accounting decode, exercised on every host.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::BTreeMap;
pub(super) fn parse(input: &Value) -> Result<BTreeMap<(u64, u32), u64>> {
    let clients = input.as_array().context("AGX client array missing")?;
    ensure!(
        !clients.is_empty(),
        "AGX per-client GPU accounting is unavailable"
    );
    let mut result = BTreeMap::new();
    for client in clients {
        let creator = client["IOUserClientCreator"]
            .as_str()
            .context("AGX client creator missing")?;
        let pid = creator
            .strip_prefix("pid ")
            .and_then(|s| s.split_once(','))
            .context("AGX PID format changed")?
            .0
            .parse::<u32>()?;
        let id = client["IORegistryEntryID"]
            .as_u64()
            .context("AGX registry identity missing")?;
        let usage = client["AppUsage"]
            .as_array()
            .context("AGX AppUsage unavailable")?;
        let mut total = 0_u64;
        for api in usage {
            total = total.saturating_add(
                api["accumulatedGPUTime"]
                    .as_u64()
                    .context("AGX accumulatedGPUTime missing")?,
            );
        }
        result.insert((id, pid), total);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn client_identity_keeps_multiple_queues_and_rejects_unmonitored_gpu_data() -> Result<()> {
        let data = serde_json::json!([{"IORegistryEntryID":123,"IOUserClientCreator":"pid 42, player","AppUsage":[{"accumulatedGPUTime":100},{"accumulatedGPUTime":30}]}]);
        assert_eq!(parse(&data)?[&(123, 42)], 130);
        assert!(parse(&serde_json::json!([])).is_err());
        assert!(parse(&serde_json::json!([{"IORegistryEntryID":123,"IOUserClientCreator":"pid 42, player"}])).is_err());
        Ok(())
    }
}
