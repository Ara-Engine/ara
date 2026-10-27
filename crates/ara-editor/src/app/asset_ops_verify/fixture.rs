//! Isolated typed source creation and canonical fixture outputs; observations never write authoring files.
use super::*;
use std::io::Read;

pub(super) struct Fixture {
    folders: String,
    populated: bool,
    cancel_job: Option<String>,
    large: PathBuf,
    ready: BTreeSet<&'static str>,
}
fn accepted(value: &Value) -> ara_ui::Result<String> {
    value["job"]
        .as_str()
        .or_else(|| {
            value["outputs"]
                .as_array()?
                .iter()
                .find_map(|output| output["job"].as_str())
        })
        .map(str::to_owned)
        .ok_or_else(|| "Fixture asset command did not return a preparation job".into())
}
fn done(editor: &mut Editor, job: &str) -> ara_ui::Result<bool> {
    let status = editor.action("jobs.status", json!({"job":job}))?;
    match status["status"].as_str() {
        Some("done") => Ok(true),
        Some("running") => Ok(false),
        _ => Err(format!(
            "Typed Assets fixture preparation failed: {}",
            ara_json::to_canonical_string(&status)
        )
        .into()),
    }
}
impl Fixture {
    pub fn new(editor: &mut Editor, output: &Path) -> ara_ui::Result<Self> {
        let output = std::path::absolute(output)?;
        let root = std::path::absolute(editor.host.root())?;
        if !root.starts_with(&output) || !root.join(".ara-behaviour-fixture").is_file() {
            return Err(
                "Assets proof may initialize only the offscreen output's scratch project".into(),
            );
        }
        std::fs::create_dir_all(&output)?;
        // Generated/package outputs are deliberately outside authored content.
        for folder in [
            ".ara/asset-proof-generated",
            "packages/asset-proof-package",
            "builds/asset-proof-output",
        ] {
            std::fs::create_dir_all(root.join(folder))?;
            std::fs::write(
                root.join(folder).join("ignored.bin"),
                b"native asset scope fixture\n",
            )?;
        }
        let preset: ara_export::ExportPreset = serde_json::from_value(
            json!({"platform":"windows","config":"dev","lane":"nativeAot","playerTemplate":"runtime","scriptProject":"","assembly":"AraGame","content":"assets","templates":[]}),
        )?;
        std::fs::create_dir_all(root.join("settings/builds"))?;
        std::fs::write(
            root.join("settings/builds/p4-native.export.json"),
            preset
                .to_unpinned_text()
                .map_err(|error| format!("{error:#}"))?,
        )?;
        let mut paths: BTreeSet<_> = ACTS.iter().map(|act| act.path()).collect();
        for suffix in [
            "Move/Destination",
            "Move Conflict/Destination",
            "Cycle/Folder/Child",
            "Cancel/Folder",
            "Index/packages/Valid",
            "Index/target/builds",
            "Index/settings/builds",
        ] {
            paths.insert(format!("{ROOT}/{suffix}"));
        }
        let commands: Vec<_> = paths
            .into_iter()
            .map(|path| json!({"cmd":"folder.create","args":{"path":path,"parents":true}}))
            .collect();
        let reply=editor.action("cmd.apply",json!({"doc":"ara.project.json","commands":commands,"why":"Create isolated native asset-operation fixture folders"}))?;
        Ok(Self {
            folders: accepted(&reply)?,
            populated: false,
            cancel_job: None,
            large: output.join("native-import-input/native-cancel.wav"),
            ready: BTreeSet::new(),
        })
    }
    pub fn prepare(
        &mut self,
        editor: &mut Editor,
        shell: &Shell,
        act: Act,
    ) -> ara_ui::Result<bool> {
        if self.ready.contains(act.folder()) {
            return Ok(true);
        }
        if !self.populated {
            if !done(editor, &self.folders)? {
                return Ok(false);
            }
            populate(editor)?;
            self.populated = true;
            return Ok(false);
        }
        if act == Act::CancelPreparation {
            if self.cancel_job.is_none() {
                if !self.large.is_file() {
                    return Err("The prior native import story's shared large WAV fixture is missing; cancellation remains unverified".into());
                }
                let reply=editor.action("cmd.apply",json!({"doc":"ara.project.json","why":"Prepare the authored source for native asset-job cancellation","commands":[{"cmd":"asset.import","args":{"sources":[self.large],"destination":format!("{ROOT}/Cancel/Folder"),"conflict":"rename"}}]}))?;
                self.cancel_job = Some(accepted(&reply)?);
                return Ok(false);
            }
            if !done(
                editor,
                self.cancel_job
                    .as_deref()
                    .ok_or("Cancel fixture job was removed")?,
            )? {
                return Ok(false);
            }
        }
        if editor.host.unsaved() {
            return Ok(false);
        }
        if storage(shell, &act.path()).is_none() {
            return Ok(false);
        }
        let ready = matches!(
            act,
            Act::Folder | Act::Index | Act::InvalidFolder | Act::CancelPreparation
        ) || storage(shell, &act.source()).is_some()
            && (!act.multi() || storage(shell, &format!("{}/B.prefab", act.path())).is_some());
        if ready {
            self.ready.insert(act.folder());
        }
        Ok(ready)
    }
}
fn populate(editor: &mut Editor) -> ara_ui::Result<()> {
    let mut documents = Vec::new();
    let mut serial = 0;
    for act in ACTS {
        if matches!(
            act,
            Act::Folder | Act::CancelPreparation | Act::InvalidFolder | Act::Index
        ) {
            continue;
        }
        for file in if act.multi() {
            vec!["A.prefab", "B.prefab"]
        } else {
            vec!["A.prefab"]
        } {
            serial += 1;
            documents.push((
                format!("{}/{file}", act.path()),
                format!("pf_aop{serial:04}"),
                Some(*act),
            ));
        }
    }
    for path in [
        "Move Conflict/Destination/B.prefab",
        "Index/packages/Valid/Nested.prefab",
        "Index/target/builds/Nested.prefab",
        "Index/settings/builds/Content.prefab",
    ] {
        serial += 1;
        documents.push((format!("{ROOT}/{path}"), format!("pf_aop{serial:04}"), None));
    }
    let commands:Vec<_>=documents.iter().map(|(path,id,_)|json!({"cmd":"doc.create","args":{"path":path,"kind":"prefab","id":id}})).collect();
    editor.action("cmd.apply",json!({"doc":"ara.project.json","commands":commands,"why":"Create canonical native Assets proof documents"}))?;
    let mut references = Vec::new();
    let mut index = 0;
    for (path, id, act) in &documents {
        editor.action("cmd.apply",json!({"doc":id,"why":"Populate an isolated Assets proof prefab","commands":[{"cmd":"entity.create","args":{"id":"e_aopcontent","name":"Native Assets proof","components":{"Transform":{},"MeshRenderer":{}}}}]}))?;
        if matches!(act, Some(Act::Rename | Act::Move | Act::Duplicate))
            || matches!(act, Some(Act::SafeDelete)) && path.ends_with("B.prefab")
        {
            index += 1;
            references.push(json!({"cmd":"prefab.instantiate","args":{"prefab":format!("@{path}#{id}"),"id":format!("e_aopref{index:04}"),"position":[50.,index as f32*2.,0.]}}));
        }
    }
    editor.apply(
        references,
        "Create inbound references for native asset safety proof",
        None,
    )?;
    Ok(())
}
pub(super) fn storage(shell: &Shell, authored: &str) -> Option<String> {
    shell
        .asset_authored_files()
        .iter()
        .find(|file| matches!(&file.scope, AuthoredScope::Authored) && file.path == authored)
        .map(|file| file.storage_path.clone())
}
pub(super) fn selection(editor: &Editor, shell: &Shell) -> Value {
    shell.asset_import_selection(editor)
}
pub(super) fn snapshot(editor: &Editor, storage: &str) -> ara_ui::Result<Value> {
    let root = std::path::absolute(editor.host.root())?;
    if storage.split('/').any(|part| part == "..") || !root.join(storage).starts_with(&root) {
        return Err("Assets observation escaped its isolated project".into());
    }
    let mut files = BTreeMap::new();
    let mut directories = Vec::new();
    walk(&root.join(storage), "", &mut files, &mut directories)?;
    let documents:serde_json::Map<_,_>=editor.host.store().workspace().docs().map(|(path,doc)|(path.to_owned(),json!({"id":doc.id,"kind":doc.kind,"text":ara_doc::write(doc,editor.host.store().schema())}))).collect();
    let mut references = BTreeSet::new();
    for (_, document) in editor.host.store().workspace().docs() {
        for record in document.records().values() {
            refs(&json!(record), &mut references);
        }
    }
    let imports: BTreeMap<_, _> = editor
        .host
        .store()
        .workspace()
        .index()
        .imports()
        .map(|(id, path)| (path.to_owned(), id.to_owned()))
        .collect();
    Ok(
        json!({"files":files,"directories":directories,"documents":documents,"references":references,"imports":imports}),
    )
}
fn walk(
    root: &Path,
    prefix: &str,
    files: &mut BTreeMap<String, Value>,
    directories: &mut Vec<String>,
) -> ara_ui::Result<()> {
    let mut entries = std::fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let relative = if prefix.is_empty() {
            entry.file_name().to_string_lossy().into_owned()
        } else {
            format!("{prefix}/{}", entry.file_name().to_string_lossy())
        };
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            return Err("Assets proof refuses a symlink in its scratch observation".into());
        }
        if kind.is_dir() {
            directories.push(relative.clone());
            walk(&entry.path(), &relative, files, directories)?;
        } else if kind.is_file() {
            let mut file = std::fs::File::open(entry.path())?;
            let mut buffer = [0; 64 * 1024];
            let mut hash = blake3::Hasher::new();
            let mut bytes = 0;
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                bytes += count;
                hash.update(&buffer[..count]);
            }
            files.insert(
                relative,
                json!({"bytes":bytes,"hash":hash.finalize().to_string()}),
            );
        }
    }
    Ok(())
}
fn refs(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(value) if value.starts_with('@') => {
            out.insert(value.clone());
        }
        Value::Array(values) => {
            for value in values {
                refs(value, out);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                refs(value, out);
            }
        }
        _ => {}
    }
}
