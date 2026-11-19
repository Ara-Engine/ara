//! Caller-owned compiler identities. Only preparation workers use this state.
//! Binding counters are derived metadata: reopening cannot reuse a retained log's
//! producer binding, while replacing a helper keeps its parent's ordered attempts.
use super::{checked_output, io};
use ara_diag::Diagnostic;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, OnceLock},
};

struct Owner {
    instance: String,
    binding: u64,
    attempt: u64,
}

/// Called with the project's publication lock held, before evaluating inputs.
pub(super) fn context(root: &Path, producer: &str) -> Result<Value, Diagnostic> {
    static OWNERS: OnceLock<Mutex<BTreeMap<(String, String), Owner>>> = OnceLock::new();
    let mut owners = OWNERS
        .get_or_init(|| Mutex::new(BTreeMap::new()))
        .lock()
        .map_err(|_| io("compiler lifecycle owner lock was poisoned"))?;
    let key = (
        ara_doc::file_state::path_key(&root.to_string_lossy()),
        producer.to_owned(),
    );
    let path = checked_output(root, ".ara/scripts/diagnostic-bindings.json")?;
    // Derived metadata: a file that does not parse (one left as zeros by a write cut short
    // before this one synced its data) starts the counters over instead of failing every
    // compile (ARA1003).
    let mut bindings: BTreeMap<String, u64> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            eprintln!(
                "warning: {} does not parse ({error}); its binding counters start over",
                path.display()
            );
            BTreeMap::new()
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
        Err(error) => return Err(io(error)),
    };
    let current = *bindings.get(producer).unwrap_or(&0);
    if owners
        .get(&key)
        .is_none_or(|owner| owner.binding != current)
    {
        let known = owners.get(&key).map_or(0, |owner| owner.binding);
        let binding = next(current.max(known))?;
        bindings.insert(producer.to_owned(), binding);
        let instance = ara_doc::IdGen::from_entropy().asset_id("cp").to_string();
        let temporary = path.with_extension(format!("json.{instance}.tmp"));
        let value = serde_json::to_value(bindings).map_err(io)?;
        // The data reaches the disk before the rename publishes it, so the file is the old one
        // or the new one, never a renamed file whose data was not yet written.
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary).map_err(io)?;
            file.write_all(ara_json::to_canonical_string(&value).as_bytes()).map_err(io)?;
            file.sync_all().map_err(io)?;
        }
        std::fs::rename(temporary, path).map_err(io)?;
        owners.insert(
            key.clone(),
            Owner {
                instance,
                binding,
                attempt: 0,
            },
        );
    }
    let owner = owners
        .get_mut(&key)
        .expect("compiler owner was initialized");
    owner.attempt = next(owner.attempt)?;
    Ok(
        json!({"producer":producer,"instance":owner.instance,"binding":owner.binding,"attempt":owner.attempt}),
    )
}

fn next(value: u64) -> Result<u64, Diagnostic> {
    value
        .checked_add(1)
        .ok_or_else(|| io("compiler lifecycle sequence exhausted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bindings file left as zeros (a write cut short before its data reached the disk) no
    /// longer fails the compile (ARA1003): the counters start over and the file is rewritten.
    #[test]
    fn a_zeroed_bindings_file_starts_its_counters_over() {
        let root = std::env::temp_dir().join(format!("ara-bindings-{}", std::process::id()));
        let scripts = root.join(".ara/scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        let file = scripts.join("diagnostic-bindings.json");
        std::fs::write(&file, [0u8; 22]).unwrap();
        let context = context(&root, "zeroed-producer").unwrap();
        assert_eq!(context["binding"], 1);
        let written: BTreeMap<String, u64> =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(written.get("zeroed-producer"), Some(&1));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
