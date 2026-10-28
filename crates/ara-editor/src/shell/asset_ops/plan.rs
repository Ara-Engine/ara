//! Pure projection of browser gestures to one atomic typed command batch.
use super::*;
pub(super) struct Plan {
    pub title: String,
    pub calls: Vec<Value>,
    pub labels: Vec<String>,
    pub changes: Vec<(assets::Asset, Option<String>)>,
    pub created: Vec<String>,
}
impl Plan {
    pub fn new(
        title: &str,
        calls: Vec<Value>,
        labels: Vec<String>,
        changes: Vec<(assets::Asset, Option<String>)>,
        select_created: bool,
    ) -> Self {
        let created = if select_created {
            changes
                .iter()
                .filter_map(|(_, destination)| destination.clone())
                .collect()
        } else {
            Vec::new()
        };
        Self {
            title: title.into(),
            calls,
            labels,
            changes,
            created,
        }
    }
}
pub(super) fn name(name: &str) -> ara_ui::Result<()> {
    if name.is_empty()
        || matches!(name, "." | "..")
        || name.contains(['/', '\\', ':'])
        || name.chars().any(char::is_control)
    {
        return Err("Enter one file or folder name without path separators".into());
    }
    Ok(())
}
pub(super) fn source(asset: &assets::Asset) -> ara_ui::Result<&str> {
    let path = asset
        .authored_path
        .as_deref()
        .ok_or("Project settings and generated/package files cannot be reorganized as assets")?;
    if path.is_empty() {
        return Err("The authored content root cannot be renamed, moved, copied or deleted".into());
    }
    Ok(path)
}
pub(super) fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}
pub(super) fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.into()
    } else {
        format!("{parent}/{name}")
    }
}
pub(super) fn delete(sources: Vec<assets::Asset>) -> ara_ui::Result<Plan> {
    let mut calls = Vec::new();
    let mut labels = Vec::new();
    let mut changes = Vec::new();
    for asset in sources {
        calls.push(json!({"cmd":"asset.delete","args":{"source":source(&asset)?}}));
        labels.push(format!("Delete {}", asset.path));
        changes.push((asset, None));
    }
    Ok(Plan::new(
        "Delete selected assets",
        calls,
        labels,
        changes,
        false,
    ))
}
pub(super) fn move_to(sources: Vec<assets::Asset>, folder: &str) -> ara_ui::Result<Plan> {
    let mut calls = Vec::new();
    let mut labels = Vec::new();
    let mut changes = Vec::new();
    for asset in sources {
        let source = source(&asset)?;
        if asset.kind == "folder"
            && (folder == source
                || folder
                    .strip_prefix(source)
                    .is_some_and(|tail| tail.starts_with('/')))
        {
            return Err("A folder cannot be moved into itself or one of its descendants".into());
        }
        let destination = join(folder, source.rsplit('/').next().unwrap_or(source));
        if destination == source {
            return Err("The selected asset is already in this folder".into());
        }
        calls.push(json!({"cmd":"asset.move","args":{"source":source,"destination":destination}}));
        labels.push(format!("Move {} to {destination}", asset.path));
        changes.push((asset, Some(destination)));
    }
    Ok(Plan::new(
        "Move selected assets",
        calls,
        labels,
        changes,
        false,
    ))
}
pub(super) fn duplicate(
    sources: Vec<assets::Asset>,
    inventory: &[assets::Asset],
) -> ara_ui::Result<Plan> {
    let mut reserved: std::collections::BTreeSet<_> = inventory
        .iter()
        .filter_map(|asset| asset.authored_path.clone())
        .collect();
    let mut calls = Vec::new();
    let mut labels = Vec::new();
    let mut changes = Vec::new();
    for asset in sources {
        let source = source(&asset)?;
        let file = source.rsplit('/').next().unwrap_or(source);
        let (stem, extension) = if asset.kind == "folder" {
            (file, None)
        } else {
            file.rsplit_once('.')
                .map_or((file, None), |(stem, ext)| (stem, Some(ext)))
        };
        let destination = (1..=10_000)
            .map(|index| {
                let suffix = if index == 1 {
                    " Copy".into()
                } else {
                    format!(" Copy {index}")
                };
                let name = if let Some(extension) = extension {
                    format!("{stem}{suffix}.{extension}")
                } else {
                    format!("{stem}{suffix}")
                };
                join(parent(source), &name)
            })
            .find(|path| !reserved.contains(path))
            .ok_or("No unused duplicate name is available")?;
        reserved.insert(destination.clone());
        calls.push(
            json!({"cmd":"asset.duplicate","args":{"source":source,"destination":destination}}),
        );
        labels.push(format!("Duplicate {} as {destination}", asset.path));
        changes.push((asset, Some(destination)));
    }
    Ok(Plan::new(
        "Duplicate selected assets",
        calls,
        labels,
        changes,
        true,
    ))
}
pub(super) fn folder(parent: &str, leaf: &str) -> ara_ui::Result<Plan> {
    name(leaf)?;
    let path = join(parent, leaf);
    Ok(Plan {
        title: "Create authored folder".into(),
        calls: vec![json!({"cmd":"folder.create","args":{"path":path}})],
        labels: vec![format!("Create {path}")],
        changes: Vec::new(),
        created: vec![path],
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn asset(path: &str, folder: bool) -> assets::Asset {
        let mut asset = assets::Asset::unindexed(
            format!("game/{path}"),
            if folder { "folder" } else { "png" }.into(),
        );
        asset.authored_path = Some(path.into());
        asset
    }
    #[test]
    fn multi_selection_moves_and_deletes_are_one_ordered_batch_with_authored_paths() {
        let sources = vec![asset("target/A.png", false), asset("builds/B.png", false)];
        let plan = move_to(sources.clone(), "packages/Art")
            .expect("nested authored folder names are unrestricted");
        assert_eq!(plan.calls.len(), 2);
        assert_eq!(plan.calls[0]["args"]["source"], "target/A.png");
        assert_eq!(plan.calls[1]["args"]["destination"], "packages/Art/B.png");
        let plan = delete(sources).expect("typed delete plan");
        assert_eq!(plan.calls.len(), 2);
        assert!(
            plan.changes
                .iter()
                .all(|(_, destination)| destination.is_none())
        );
    }
    #[test]
    fn folder_cycles_and_project_scope_are_rejected_before_a_gesture_can_commit() {
        assert!(move_to(vec![asset("Art", true)], "Art/Children").is_err());
        assert!(move_to(vec![asset("Art", true)], "Art").is_err());
        assert!(source(&assets::Asset::folder("settings/builds".into(), None)).is_err());
        assert!(source(&assets::Asset::folder("game".into(), Some(String::new()))).is_err());
    }
    #[test]
    fn duplicate_destinations_are_reserved_for_the_whole_batch() {
        let selected = vec![asset("Art/a.png", false), asset("Art/a Copy.png", false)];
        let plan = duplicate(selected.clone(), &selected).expect("duplicate plan");
        assert_eq!(plan.calls[0]["args"]["destination"], "Art/a Copy 2.png");
        assert_eq!(plan.calls[1]["args"]["destination"], "Art/a Copy Copy.png");
    }
}
