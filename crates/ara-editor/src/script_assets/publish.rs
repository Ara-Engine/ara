//! A successful compiler image publishes its actual schema before attachment.
use super::*;

pub(super) fn complete(
    cx: &mut Cx<'_>,
    session: &str,
    path: &str,
    doc: Option<&str>,
    attach: &[String],
    result: Value,
) -> Result<Value, Vec<ara_diag::Diagnostic>> {
    if result["ok"] != true {
        return Err(serde_json::from_value(result["diagnostics"].clone())
            .unwrap_or_else(|_| vec![crate::invalid("C# compilation failed")]));
    }
    cx.resume_session(session, |cx| publish(cx, path, doc, attach, &result))?
}

fn publish(
    cx: &mut Cx<'_>,
    path: &str,
    doc: Option<&str>,
    attach: &[String],
    result: &Value,
) -> Result<Value, Vec<ara_diag::Diagnostic>> {
    let types = result["types"]
        .as_object()
        .ok_or_else(|| vec![crate::invalid("Compile returned no reflected types")])?;
    let mut schema = cx.store().schema().clone();
    for document in types.values() {
        if let Some(definitions) = document["types"].as_object() {
            for (name, definition) in definitions {
                if let Some(existing) = schema.get(name) {
                    let incoming =
                        ara_schema::type_from_json(name, definition).map_err(|d| vec![d])?;
                    if incoming.module != existing.module {
                        return Err(vec![crate::invalid(format!(
                            "C# type {name} conflicts with an existing type"
                        ))]);
                    }
                    schema.remove(name);
                }
            }
        }
        let errors = schema
            .add_json_types(document)
            .into_iter()
            .filter(ara_diag::Diagnostic::is_error)
            .collect::<Vec<_>>();
        if !errors.is_empty() {
            return Err(errors);
        }
    }
    let errors = schema
        .verify()
        .into_iter()
        .filter(ara_diag::Diagnostic::is_error)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(errors);
    }
    cx.transact(Some("Publish compiled C# types".into()), |store, _| {
        *store.schema_mut() = schema;
    })?;
    // Reflection is derived output, canonical and generated from the compiled image.
    for module in crate::shell::scripts::modules(cx.root()) {
        let spec = ara_script::ModuleSpec::from_dir(&module).map_err(|d| vec![d])?;
        if let Some(types) = types.get(&spec.name) {
            std::fs::write(
                module.join(format!("{}.types.json", spec.name)),
                ara_json::to_canonical_string(types),
            )
            .map_err(|e| vec![crate::invalid(e.to_string())])?;
        }
    }
    let component = if path.is_empty() {
        ""
    } else {
        script_name(path).map_err(|e| vec![crate::invalid(e)])?
    };
    let transaction = if attach.is_empty() {
        Value::Null
    } else {
        let commands=attach.iter().map(|entity|json!({"cmd":"component.add","args":{"entity":entity,"component":component}})).collect::<Vec<_>>();
        cx.apply(parse(json!({"session":cx.session().map(|s|s.id.as_str()),"doc":doc,"why":format!("Attach {component}"),"commands":commands}))?)?
    };
    Ok(
        json!({"path":path,"compiled":true,"component":component,"attachedTo":attach,"transaction":transaction}),
    )
}
