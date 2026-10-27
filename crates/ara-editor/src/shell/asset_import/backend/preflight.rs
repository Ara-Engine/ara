//! A read-only command adapter; source/destination planning stays in the authority.
use super::super::Request;
use crate::Editor;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One existing destination reported by the authoritative import planner.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Conflict {
    /// Absolute OS source involved in the conflict.
    pub source: String,
    /// Authored relative destination path, independent of browser storage layout.
    #[serde(rename = "destinationLogicalPath")]
    pub destination_logical_path: String,
    /// Existing stable asset identity, when the destination already has one.
    #[serde(default, rename = "existingAssetId")]
    pub existing_asset_id: Option<String>,
    /// Planner's description of the existing destination kind.
    pub kind: String,
}
impl Conflict {
    fn label(&self) -> String {
        let source = std::path::Path::new(&self.source)
            .file_name()
            .map(|name| name.to_string_lossy())
            .unwrap_or_else(|| self.source.as_str().into());
        format!(
            "{source} → {} ({})",
            self.destination_logical_path, self.kind
        )
    }
}
/// Read-only conflict inventory; planned entries remain authority-owned data.
#[derive(Debug, Deserialize, Serialize)]
pub(in crate::shell::asset_import) struct Plan {
    /// Existing destinations, in authoritative planner order.
    conflicts: Vec<Conflict>,
    /// Planned entries are preserved for evidence without duplicating their schema.
    planned: Vec<Value>,
}
impl Plan {
    pub fn read(value: &Value) -> ara_ui::Result<Self> {
        let inventory = if value.get("conflicts").is_some() {
            value
        } else {
            value["outputs"]
                .as_array()
                .and_then(|outputs| outputs.first())
                .ok_or("Import preflight did not return a conflict inventory")?
        };
        let plan: Self = serde_json::from_value(inventory.clone())
            .map_err(|error| format!("Invalid import preflight inventory: {error}"))?;
        for conflict in &plan.conflicts {
            if conflict.source.is_empty()
                || conflict.source.contains('\0')
                || conflict.destination_logical_path.is_empty()
                || !super::authored_folder(&conflict.destination_logical_path)
                || conflict.kind.trim().is_empty()
                || conflict
                    .existing_asset_id
                    .as_ref()
                    .is_some_and(|id| id.trim().is_empty())
            {
                return Err(
                    "Import preflight returned an invalid source or destination conflict".into(),
                );
            }
        }
        Ok(plan)
    }
    pub fn labels(&self) -> Vec<String> {
        self.conflicts.iter().map(Conflict::label).collect()
    }
    pub fn value(&self) -> ara_ui::Result<Value> {
        Ok(serde_json::to_value(self)?)
    }
}
/// A preflight can reply immediately or prepare its inventory on a host job.
pub(in crate::shell::asset_import) enum Preflight {
    Ready(Plan),
    Pending(String),
}

pub(in crate::shell::asset_import) fn preflight(
    editor: &mut Editor,
    request: &Request,
) -> ara_ui::Result<Preflight> {
    let response = editor.action("cmd.apply", request_params(request)?)?;
    read_response(&response)
}
fn request_params(request: &Request) -> ara_ui::Result<Value> {
    let sources = request
        .sources
        .iter()
        .map(|path| {
            path.to_str()
                .ok_or("An import source cannot be represented as UTF-8")
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"doc":"ara.project.json",
    "why":"Check imported file destinations","commands":[{"cmd":"asset.import","args":{
        "sources":sources,"destination":request.destination,"conflict":"rename","dryRun":true
    }}]}))
}
fn read_response(response: &Value) -> ara_ui::Result<Preflight> {
    if response["ok"] == false {
        return Err("Import preflight was not accepted by the authority".into());
    }
    if let Some(job) = super::accepted_job(response) {
        return Ok(Preflight::Pending(job.into()));
    }
    if response.get("job").is_some()
        || response["outputs"]
            .as_array()
            .is_some_and(|outputs| outputs.iter().any(|output| output.get("job").is_some()))
    {
        return Err("Import preflight returned an invalid background job identity".into());
    }
    Ok(Preflight::Ready(Plan::read(response)?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn preflight_uses_the_authority_read_only_job_instead_of_rolling_back_a_started_job() {
        let source = std::env::temp_dir().join("outside.wav");
        let request = Request {
            sources: vec![source.clone()],
            destination: "Custom Content/Audio".into(),
            conflicts: Vec::new(),
        };
        let params = request_params(&request).unwrap();
        assert_eq!(params["doc"], "ara.project.json");
        assert!(params.get("dryRun").is_none());
        assert_eq!(params["commands"][0]["cmd"], "asset.import");
        assert_eq!(
            params["commands"][0]["args"],
            json!({"sources":[source.to_str().unwrap()],"destination":"Custom Content/Audio","conflict":"rename","dryRun":true})
        );
    }
    #[test]
    fn root_and_batch_conflicts_are_typed_and_empty_inventories_are_valid() {
        let inventory = json!({"conflicts":[{"source":"/outside/rock.glb","destinationLogicalPath":"Art/rock.glb","existingAssetId":"as_rock","kind":"file"}],"planned":[{"path":"Art/rock-2.glb"}]});
        for response in [inventory.clone(), json!({"outputs":[inventory]})] {
            let Preflight::Ready(plan) = read_response(&response).unwrap() else {
                panic!("Immediate inventory expected")
            };
            assert_eq!(plan.labels(), ["rock.glb → Art/rock.glb (file)"]);
            assert_eq!(
                plan.value().unwrap()["conflicts"][0]["existingAssetId"],
                "as_rock"
            );
        }
        let Preflight::Ready(plan) = read_response(&json!({"conflicts":[],"planned":[]})).unwrap()
        else {
            panic!("Empty inventory is ready")
        };
        assert!(plan.labels().is_empty());
        assert!(
            read_response(&json!({"ok":true})).is_err(),
            "missing inventory must not silently import"
        );
    }
    #[test]
    fn a_preparing_job_is_awaited_even_if_its_reply_contains_placeholder_arrays() {
        let response =
            json!({"outputs":[{"job":"jb_plan","preparing":true,"conflicts":[],"planned":[]}]});
        assert!(
            matches!(read_response(&response).unwrap(),Preflight::Pending(job) if job=="jb_plan")
        );
    }
    #[test]
    fn completed_job_inventory_is_the_exact_camel_case_first_command_output() {
        let status = json!({"status":"done","cancellable":false,"result":{"outputs":[{"dryRun":true,"conflicts":[{"source":"/outside/a.wav","destinationLogicalPath":"Audio/a.wav","existingAssetId":"as_existing","kind":"file"}],"planned":[{"destinationLogicalPath":"Audio/a-2.wav"}]}]}});
        let plan = Plan::read(&status["result"]).unwrap();
        assert_eq!(
            plan.value().unwrap()["conflicts"][0]["destinationLogicalPath"],
            "Audio/a.wav"
        );
        assert_eq!(
            plan.value().unwrap()["conflicts"][0]["existingAssetId"],
            "as_existing"
        );
        assert!(
            Plan::read(&status).is_err(),
            "jobs.status must be unwrapped at result before reading outputs[0]"
        );
    }
    #[test]
    fn malformed_inventories_cannot_bypass_the_conflict_choice() {
        for response in [
            json!({"ok":false,"conflicts":[],"planned":[]}),
            json!({"job":"","conflicts":[],"planned":[]}),
            json!({"conflicts":null,"planned":[]}),
            json!({"conflicts":[]}),
            json!({"conflicts":[{"source":"/outside/a.wav","destinationLogicalPath":"../a.wav","kind":"file"}],"planned":[]}),
            json!({"conflicts":[{"source":"","destinationLogicalPath":"Audio/a.wav","kind":"file"}],"planned":[]}),
            json!({"conflicts":[{"source":"/outside/a.wav","destinationLogicalPath":"Audio/a.wav","kind":""}],"planned":[]}),
        ] {
            assert!(read_response(&response).is_err(), "{response}");
        }
    }
}
