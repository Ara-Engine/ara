//! Compact canonical snapshots and effect evidence; no complete scene/UI serialization.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn documents(editor: &Editor) -> Value {
    let store = editor.host.store();
    let documents:BTreeMap<_,_>=store.workspace().docs().map(|(path,document)| {
        let text=ara_doc::write(document,store.schema());
        (path,json!({"canonicalBytes":text.len(),"blake3":blake3::hash(text.as_bytes()).to_hex().to_string()}))
    }).collect();
    json!(documents)
}

pub(super) fn history(editor: &mut Editor) -> ara_ui::Result<Vec<Value>> {
    let Value::Object(mut reply) = editor.action("editor.history", json!({}))? else {
        return Err("Editor navigation history returned no object".into());
    };
    match reply.remove("items") {
        Some(Value::Array(items)) => Ok(items),
        _ => Err("Editor navigation history returned no transaction array".into()),
    }
}

fn geometry(shell: &Shell, id: &str) -> Option<Value> {
    let index = shell.shown_node(id)?;
    let node = &shell.ui.nodes[index];
    Some(
        json!({"id":id,"tag":node.element.tag,"rect":node.rect,"clip":node.clip,
        "label":node.element.attrs.get("label"),"text":node.text.as_ref().map(|text|&text.value),"enabled":!node.state.disabled}),
    )
}

impl BookmarksCheck {
    pub(super) fn unchanged(&self, editor: &Editor) -> ara_ui::Result<()> {
        if editor.host.store().revision() != self.revision
            || editor.state.doc != self.doc
            || self.camera != Some(editor.state.camera)
            || editor.state.selection != self.selection
        {
            return Err(format!("Bookmark navigation changed protected context: content revision {}->{}, doc {}->{}, cameraPreserved={}, selectionPreserved={}",self.revision,editor.host.store().revision(),self.doc,editor.state.doc,self.camera==Some(editor.state.camera),editor.state.selection==self.selection).into());
        }
        Ok(())
    }

    pub(super) fn mark(
        &mut self,
        stage: &'static str,
        editor: &Editor,
        shell: &Shell,
        effect: Value,
    ) {
        self.trace.push(json!({"stage":stage,"phase":self.phase,"effect":effect,
            "contentRevision":editor.host.store().revision(),"folder":editor.state.asset_folder,
            "query":editor.state.searches.get("assets"),"folderOnly":editor.state.asset_search_folder_only,
            "inspectedAsset":editor.state.asset_path,"favourite":editor.state.asset_favourites.get(&self.folder),
            "savedSearch":editor.state.asset_saved_searches.get(SAVED),"modal":shell.modal_active(),"selectOpen":shell.choice_open(),
            "documentPreserved":editor.state.doc==self.doc,"cameraPreserved":self.camera==Some(editor.state.camera),
            "selectionPreserved":editor.state.selection==self.selection}));
        self.pending_capture = Some(stage);
    }

    pub(super) fn check_navigation(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.unchanged(editor)?;
        let after = documents(editor);
        let history = history(editor)?;
        let Some(items) = history.get(self.nav_start..) else {
            return Err("Navigation history shrank during bookmark gestures".into());
        };
        let expected = [
            "editor.workspace",
            "editor.search",
            "editor.workspace",
            "editor.workspace",
            "editor.workspace",
            "editor.search",
            "editor.workspace",
            "editor.workspace",
            "editor.workspace",
        ];
        let ids: BTreeSet<_> = items
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect();
        let single = items.len() == expected.len()
            && ids.len() == items.len()
            && items.iter().zip(expected).all(|(item, label)| {
                item["author"] == "human"
                    && item["label"] == label
                    && item["ops"].as_array().is_some_and(|ops| ops.len() == 1)
            });
        let compact:Vec<_>=items.iter().map(|item|json!({"id":item["id"],"author":item["author"],"label":item["label"],"why":item["why"],"opCount":item["ops"].as_array().map(Vec::len)})).collect();
        self.navigation = json!({"ok":single && after==self.documents,"beforeCount":self.nav_start,"afterCount":history.len(),
            "expectedIntentCount":expected.len(),"singleHumanOpPerIntent":single,"expectedLabels":expected,"transactions":compact,
            "contentUnchanged":after==self.documents,"afterContentRevision":editor.host.store().revision(),"afterDocuments":after});
        if !single {
            return Err(format!("Bookmark gestures did not produce exactly one typed human navigation op per intent: {}",ara_json::to_canonical_string(&self.navigation)).into());
        }
        if self.navigation["contentUnchanged"] != true {
            return Err("Bookmark navigation changed canonical project content".into());
        }
        Ok(())
    }

    pub(super) fn capture(
        &mut self,
        stage: &str,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        shell.leave();
        gpu.select_target(0);
        let modal = shell.ui.paint_order.iter().copied().rev().find(|index| {
            let node = &shell.ui.nodes[*index];
            shell.ui.shown(*index)
                && node.element.id.starts_with("editor-confirm-")
                && node.element.id.ends_with("-body")
        });
        let index = modal
            .or_else(|| shell.shown_node("choice-asset-bookmark-select-popover"))
            .or_else(|| shell.shown_node("body-assets"));
        let (mut rect, surface) = if let Some(index) = index {
            let node = &shell.ui.nodes[index];
            (node.rect.intersection(node.clip), node.element.id.as_str())
        } else if stage == "failure" {
            (
                ara_ui::paint::Box2 {
                    x: 0.,
                    y: 0.,
                    w: shell.ui.size[0],
                    h: shell.ui.size[1],
                },
                "failure-main-viewport",
            )
        } else {
            return Err("Bookmarks capture has no relevant Assets/modal/popup surface".into());
        };
        // Keep ordinary captures to the toolbar and first usable rows, while an
        // actual shared modal/Select is captured in full at its own bounded width.
        if surface == "body-assets" {
            rect.h = rect.h.min(400.);
        }
        let dpi = gpu.dpi();
        let pixels = gpu.pixels()?;
        let x = (rect.x * dpi).round().max(0.) as u32;
        let y = (rect.y * dpi).round().max(0.) as u32;
        let w = ((rect.w * dpi).round().max(0.) as u32).min(pixels.width().saturating_sub(x));
        let h = ((rect.h * dpi).round().max(0.) as u32).min(pixels.height().saturating_sub(y));
        if w == 0 || h == 0 {
            return Err("Bookmarks proof would capture an empty panel".into());
        }
        std::fs::create_dir_all(&self.output)?;
        let file = format!("bookmarks-{stage}.png");
        image::imageops::crop_imm(&pixels, x, y, w, h)
            .to_image()
            .save(self.output.join(&file))?;
        let controls: Vec<_> = [
            "assets-favourite-toggle",
            "assets-search-save",
            "search-assets",
            "asset-search-folder-only",
            "asset-bookmark-select",
            "asset-grid",
            "choice-asset-bookmark-select-search",
        ]
        .into_iter()
        .filter_map(|id| geometry(shell, id))
        .collect();
        let options: Vec<_> = shell
            .ui
            .paint_order
            .iter()
            .copied()
            .filter(|index| {
                shell.ui.shown(*index)
                    && shell.ui.nodes[*index]
                        .element
                        .id
                        .starts_with("choice-asset-bookmark-select-option-")
            })
            .take(8)
            .filter_map(|index| geometry(shell, &shell.ui.nodes[index].element.id))
            .collect();
        let metadata = json!({"stage":stage,"file":file,"phase":self.phase,"dpi":dpi,"logicalRect":rect,"pixelRect":[x,y,w,h],
            "surface":"main","cropSource":surface,"contentRevision":editor.host.store().revision(),"controls":controls,"options":options,
            "focus":shell.input.focused().and_then(|id|shell.ui.by_id(id)).map(|index|&shell.ui.nodes[index].element.id),
            "folder":editor.state.asset_folder,"query":editor.state.searches.get("assets"),"folderOnly":editor.state.asset_search_folder_only});
        std::fs::write(
            self.output.join(format!("bookmarks-{stage}.json")),
            ara_json::to_canonical_string(&metadata),
        )?;
        if let Some(trace) = self
            .trace
            .iter_mut()
            .rev()
            .find(|trace| trace["stage"] == stage)
        {
            trace["capture"] = metadata;
        }
        Ok(())
    }

    pub(super) fn finish_results(&mut self) {
        let guard = self.error.is_none() && self.restored && self.navigation["ok"] == true;
        self.results = vec![
            json!({"journey":FAVOURITE,"ok":guard && self.favourite_recalled,"nativeFolderToggleAndKeyboardSelectRecall":self.favourite_recalled,"error":self.error}),
            json!({"journey":SEARCH,"ok":guard && self.search_recalled,"sharedPromptAndPointerSelectRecall":self.search_recalled,"exactScopedQueryRestored":self.search_recalled,"error":self.error}),
        ];
    }
}
