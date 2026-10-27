//! Every dependency graph node is a physical browser key; references remain logical.
use super::*;
pub(super) fn document<'a>(
    workspace: &'a ara_doc::Workspace,
    asset: &assets::Asset,
) -> Option<&'a ara_doc::Document> {
    let logical = asset.authored_path.as_deref().unwrap_or(&asset.path);
    workspace
        .find(logical)
        .and_then(|id| workspace.doc(id.as_str()))
        .filter(|doc| workspace.path_of(doc.id.as_str()) == Some(asset.path.as_str()))
        .or_else(|| {
            workspace
                .docs()
                .find_map(|(storage, doc)| (storage == asset.path).then_some(doc))
        })
}
pub(super) fn dependency(
    files: &[ara_host::project::AuthoredFile],
    workspace: &ara_doc::Workspace,
    reference: &str,
) -> Option<String> {
    let parts = ara_schema::grammar::parse_ref(reference).ok()?;
    let logical = parts
        .id
        .and_then(|id| workspace.index().path_of(id))
        .or(parts.path)?;
    Some(
        ops::authored::existing_storage(files, logical)
            .map_or_else(|| workspace.storage_path(logical), str::to_owned),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_host::project::{AuthoredFile, AuthoredRole, AuthoredScope};
    #[test]
    fn project_and_authored_documents_with_identical_logical_names_keep_distinct_nodes() {
        let logical = "settings/builds/same.graph";
        let mut workspace = ara_doc::Workspace::default();
        workspace.set_reference_root(ara_doc::AUTHORING_ROOT);
        let storage = workspace.storage_path(logical);
        workspace.insert_project_document(
            logical,
            ara_doc::Document::new(
                "graph",
                ara_doc::AssetId::parse("gr_project").expect("fixture ID"),
            ),
        );
        workspace.insert(
            storage.clone(),
            ara_doc::Document::new(
                "graph",
                ara_doc::AssetId::parse("gr_authored").expect("fixture ID"),
            ),
        );
        let files = vec![
            AuthoredFile {
                path: logical.into(),
                storage_path: logical.into(),
                scope: AuthoredScope::Project,
                role: AuthoredRole::Document {
                    name: "graph".into(),
                },
            },
            AuthoredFile {
                path: logical.into(),
                storage_path: storage.clone(),
                scope: AuthoredScope::Authored,
                role: AuthoredRole::Document {
                    name: "graph".into(),
                },
            },
        ];
        let mut authored = assets::Asset::unindexed(storage.clone(), "graph".into());
        authored.authored_path = Some(logical.into());
        let project = assets::Asset::unindexed(logical.into(), "graph".into());
        assert_eq!(
            document(&workspace, &authored).map(|doc| doc.id.as_str()),
            Some("gr_authored")
        );
        assert_eq!(
            document(&workspace, &project).map(|doc| doc.id.as_str()),
            Some("gr_project")
        );
        let target = dependency(&files, &workspace, "@old.graph#gr_authored")
            .expect("indexed authored target");
        assert_eq!(target, storage);
        let mut graph = ara_asset::DependencyGraph::default();
        graph.replace(project.path.clone(), [target]);
        assert_eq!(
            graph.users(&authored.path).collect::<Vec<_>>(),
            vec![project.path.as_str()]
        );
        assert!(graph.users(&project.path).next().is_none());
    }
}
