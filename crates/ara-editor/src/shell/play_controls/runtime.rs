use super::*;

/// The sole adapter to current synchronous host methods. A deferred host job can
/// replace this boundary without changing field generation or typed set creation.
fn call(editor: &mut Editor, method: &str, params: Value) -> ara_ui::Result<Value> {
    Ok(editor.action(method, params)?)
}
impl Shell {
    pub(crate) fn inspect_play_selection(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        if self
            .play_controls
            .runtime
            .as_ref()
            .is_some_and(|runtime| !runtime.drafts.is_empty())
        {
            return Err("Apply or discard pending runtime edits before refreshing".into());
        }
        let world = editor
            .play_world
            .as_deref()
            .ok_or("Start Play before inspecting runtime values")?
            .to_owned();
        let entity = editor
            .state
            .selection
            .first()
            .ok_or("Select an entity in Hierarchy before inspecting it")?
            .clone();
        let inspected = call(
            editor,
            "play.inspect",
            json!({"world":world,"player":self.game_client,"entity":entity}),
        )?;
        let status = call(editor, "play.status", json!({}))?;
        let local = status["worlds"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["world"] == world)
            .and_then(|row| row["players"].as_array())
            .and_then(|players| players.first())
            .is_some_and(|player| player["role"] == "local");
        let components = inspected
            .get("components")
            .filter(|value| value.is_object())
            .cloned()
            .ok_or("Runtime inspection did not return component values")?;
        self.play_controls.runtime = Some(Runtime {
            world,
            player: self.game_client,
            entity,
            tick: inspected["tick"].as_u64().unwrap_or(0),
            components,
            local,
            drafts: BTreeMap::new(),
        });
        self.play_controls.options = true;
        self.play_controls.message = None;
        self.rebuild(editor)
    }
    /// Native fields stage values; Enter/Apply emits one tick-boundary patch. Fresh
    /// values preserve unrelated channels, and changed positional collections refuse stale edits.
    pub(crate) fn commit_play_fields(&mut self, editor: &mut Editor) -> ara_ui::Result<bool> {
        let Some(mut runtime) = self.play_controls.runtime.take() else {
            return Ok(false);
        };
        let result = (|| -> ara_ui::Result<bool> {
            if runtime.drafts.is_empty() {
                return Ok(false);
            }
            if !runtime.local {
                return Err("Live editing is available in Local Play. Networked clients support inspection and keeping authority changes".into());
            }
            if editor.play_world.as_deref() != Some(runtime.world.as_str())
                || self.game_client != runtime.player
                || editor.state.selection.first().map(String::as_str)
                    != Some(runtime.entity.as_str())
            {
                return Err("The inspected Play world, client or selection changed; inspect the entity again".into());
            }
            let fresh = call(
                editor,
                "play.inspect",
                json!({"world":runtime.world,"player":runtime.player,"entity":runtime.entity}),
            )?;
            let mut values = BTreeMap::<String, Value>::new();
            for draft in runtime.drafts.values() {
                let (component, field) = draft
                    .target
                    .path
                    .split_once('.')
                    .ok_or("Invalid runtime field path")?;
                let live = fresh["components"]
                    .get(component)
                    .and_then(|value| value.get(field))
                    .ok_or("The runtime field was removed; inspect again")?;
                let before = runtime
                    .components
                    .get(component)
                    .and_then(|value| value.get(field))
                    .ok_or("The inspected field disappeared")?;
                if positional_parent_changed(before, live, &draft.target.pointer) {
                    return Err("This runtime collection changed while editing. Inspect it again before applying the positional edit".into());
                }
                let value = values
                    .entry(draft.target.path.clone())
                    .or_insert_with(|| live.clone());
                let slot = if draft.target.pointer.is_empty() {
                    value
                } else {
                    value
                        .pointer_mut(&draft.target.pointer)
                        .ok_or("The runtime collection item was removed")?
                };
                *slot = parse(&draft.target, &draft.input)?;
            }
            let sets: Vec<_> = values
                .into_iter()
                .map(|(path, value)| json!({"entity":runtime.entity,"path":path,"value":value}))
                .collect();
            let response = call(
                editor,
                "play.patch",
                json!({"world":runtime.world,"sets":sets}),
            )?;
            let inspected = call(
                editor,
                "play.inspect",
                json!({"world":runtime.world,"player":runtime.player,"entity":runtime.entity}),
            )?;
            runtime.components = inspected["components"].clone();
            runtime.tick = inspected["tick"].as_u64().unwrap_or(runtime.tick);
            runtime.drafts.clear();
            self.play_controls.message = Some(format!(
                "Runtime edits applied at tick {} to all players. Keep selected changes to save them to the scene.",
                response["appliedAtTick"].as_u64().unwrap_or(runtime.tick)
            ));
            Ok(true)
        })();
        if let Err(error) = &result {
            self.play_controls.message = Some(error.to_string());
        }
        self.play_controls.runtime = Some(runtime);
        result
    }
    /// Keep selected runtime fields through PlayService's conflict-checked typed content batch.
    pub(crate) fn keep_play_changes(
        &mut self,
        editor: &mut Editor,
        include_driven: bool,
    ) -> ara_ui::Result<()> {
        if editor.state.selection.is_empty() {
            return Err("Select the entities whose runtime changes should be kept".into());
        }
        if self
            .play_controls
            .runtime
            .as_ref()
            .is_some_and(|runtime| !runtime.drafts.is_empty())
        {
            return Err("Apply or discard pending runtime edits before keeping changes".into());
        }
        let world = editor
            .play_world
            .as_deref()
            .ok_or("Start Play before keeping runtime changes")?
            .to_owned();
        let params = json!({"world":world,"entities":editor.state.selection,"includeDriven":include_driven,"why":"Keep selected runtime changes"});
        let result = call(editor, "play.keep", params)?;
        let count = |key: &str| result[key].as_array().map_or(0, Vec::len);
        self.play_controls.message = Some(format!(
            "Kept {} fields · {} conflicts left unchanged · {} unsupported changes skipped. One Undo restores the kept scene values.",
            count("kept"),
            count("conflicts"),
            count("skipped")
        ));
        if count("conflicts") > 0 {
            self.dialog=Some(Dialog::Info {title:"Runtime changes conflict with scene edits".into(),lines:vec!["Conflicting scene values were preserved. The runtime and scene values are listed below.".into()],code:Some(ara_json::to_canonical_string(&result["conflicts"]))});
        }
        self.rebuild(editor)
    }
}
fn parse(target: &FieldTarget, input: &Value) -> ara_ui::Result<Value> {
    let Some(text) = input.as_str() else {
        return Ok(input.clone());
    };
    if let Some((min, max)) = target.ty.int_bounds() {
        let value = text
            .trim()
            .parse::<i128>()
            .map_err(|_| "Enter a whole number")?;
        if value < min || value > max {
            return Err("The whole number is outside this field's range".into());
        }
        return Ok(if value >= 0 {
            json!(value as u64)
        } else {
            json!(value as i64)
        });
    }
    if target.ty.is_number() {
        return Ok(json!(crate::inspector::parse_number(
            text,
            target.unit.as_deref()
        )?));
    }
    if matches!(target.ty, ValueType::Entity | ValueType::Asset(_)) && text.trim().is_empty() {
        return Ok(Value::Null);
    }
    Ok(json!(text))
}
fn positional_parent_changed(before: &Value, after: &Value, pointer: &str) -> bool {
    let mut prefix = String::new();
    for segment in pointer
        .strip_prefix('/')
        .into_iter()
        .flat_map(|pointer| pointer.split('/'))
    {
        let left = if prefix.is_empty() {
            Some(before)
        } else {
            before.pointer(&prefix)
        };
        let right = if prefix.is_empty() {
            Some(after)
        } else {
            after.pointer(&prefix)
        };
        if left.is_some_and(Value::is_array) && left != right {
            return true;
        }
        prefix.push('/');
        prefix.push_str(segment);
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positional_edits_reject_reordered_arrays_but_preserve_other_map_entries() {
        assert!(positional_parent_changed(
            &json!([{"v":1},{"v":2}]),
            &json!([{"v":2},{"v":1}]),
            "/0/v"
        ));
        assert!(!positional_parent_changed(
            &json!({"a":1,"b":2}),
            &json!({"a":1,"b":3}),
            "/a"
        ));
    }
    #[test]
    fn runtime_integer_text_keeps_full_u64_precision_and_rejects_fractional_input() {
        let target = FieldTarget {
            path: "Counter.value".into(),
            pointer: String::new(),
            ty: ValueType::U64,
            unit: None,
        };
        assert_eq!(
            parse(&target, &json!("18446744073709551615")).ok(),
            Some(json!(u64::MAX))
        );
        assert!(parse(&target, &json!("1.5")).is_err());
        assert!(parse(&target, &json!("-1")).is_err());
    }
}
