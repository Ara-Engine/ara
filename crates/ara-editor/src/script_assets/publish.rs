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
    let mut diagnostics:Vec<ara_diag::Diagnostic>=match result.get("diagnostics") {
        Some(value)=>serde_json::from_value(value.clone()).map_err(|error| {
            let diagnostic=crate::invalid(format!("Compiler diagnostics could not be decoded: {error}"));
            cx.log(diagnostic.clone());
            vec![diagnostic]
        })?,
        None=>Vec::new(),
    };
    // Lifecycle Info and warnings are authoritative even when compilation fails.
    // Jobs must not log this already-published envelope a second time.
    for diagnostic in &diagnostics {cx.log(diagnostic.clone());}
    if diagnostics.iter().any(ara_diag::Diagnostic::is_error) {
        return Err(diagnostics);
    }
    if result["ok"] != true {
        let diagnostic=crate::invalid("C# compiler reported failure without an error diagnostic");
        cx.log(diagnostic.clone());
        diagnostics.push(diagnostic);
        return Err(diagnostics);
    }
    let published=cx.resume_session(session, |cx| publish(cx, path, doc, attach, &result))
        .and_then(|result|result);
    match published {
        Ok(mut result)=>{
            result["diagnostics"]=json!(diagnostics);
            Ok(result)
        }
        Err(failure)=>{
            for diagnostic in &failure {cx.log(diagnostic.clone());}
            diagnostics.extend(failure);
            Err(diagnostics)
        }
    }
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
    // The compile worker published canonical schemas into the explicit derived
    // inventory. The authority only adopts their types and applies attachment ops.
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
