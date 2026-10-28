//! Check published evidence, never infer retirement from log message text.
use super::*;

pub(super) fn inventory(projection: &Value, before: &Value) -> Result<(), String> {
    if projection["schema"] != 1
        || projection.get("readError") != Some(&Value::Null)
        || projection["unknownEarlierScopes"] != false
    {
        return Err(
            "Compile projection is missing, has a read error, or has unknown earlier scopes".into(),
        );
    }
    for field in ["omittedScopeEvents", "omittedRows", "omittedProtectedRows"] {
        if projection[field] != 0 {
            return Err(format!("Compile projection has missing inventory: {field}"));
        }
    }
    if number(&projection["invalidMetadata"])? > number(&before["invalidMetadata"])? {
        return Err("Compiler metadata became invalid during the fixed attempt".into());
    }
    if !projection["scopes"].is_array() || !projection["protectedRuntimeFaultSeqs"].is_array() {
        return Err("Compile projection omitted scope/runtime inventory".into());
    }
    Ok(())
}

pub(super) fn scope(target: &Target, projection: &Value) -> Result<Value, String> {
    let scopes = projection["scopes"]
        .as_array()
        .ok_or("Compile scope inventory missing")?;
    let matching = scopes
        .iter()
        .filter(|scope| scope["key"] == target.key)
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Err("Exact producer/profile/module/world/player scope absent or ambiguous".into());
    }
    let scope = matching[0];
    let latest = order(&scope["order"])?;
    let covered = order(&scope["covered"])?;
    let verified = &scope["lastVerifiedCompletion"];
    if latest <= target.order || latest != covered || latest != order(&verified["order"])? {
        return Err(
            "No newer matching verified completion for the current numeric binding/attempt".into(),
        );
    }
    let bound = &scope["bound"];
    let binding = (
        number(&bound["playBinding"])?,
        number(&bound["compileBinding"])?,
    );
    if binding != (latest.0, latest.1) {
        return Err("Completion belongs to a stale compiler binding".into());
    }
    let instance = token(&bound["instance"])?;
    let source = match bound.get("source") {
        Some(Value::Null) => None,
        Some(source) => Some(token(source)?),
        None => return Err("Current compiler binding omitted its source identity".into()),
    };
    if binding == (target.order.0, target.order.1)
        && (instance != target.instance || source != target.source)
    {
        return Err("Opaque source/instance changed without a newer numeric binding".into());
    }
    if verified["result"] != "success"
        || verified["validation"] != "current-module-inputs"
        || scope["phase"] != "success"
        || scope["pending"] != false
        || scope["incomplete"] != false
        || scope["unknown"] != false
        || scope["omittedCurrentRows"] != false
        || scope["errors"] != 0
        || scope["activeErrorSeqs"]
            .as_array()
            .is_none_or(|rows| !rows.is_empty())
    {
        return Err("Current scope is pending, uncertain or still has active errors".into());
    }
    let attempt = &scope["attempt"];
    if attempt["started"] != true
        || attempt["tainted"] != false
        || attempt["unknownCodeFamily"] != false
        || attempt["verified"] != true
        || attempt["truncated"] != false
        || attempt["result"] != "success"
        || attempt["observedErrors"] != 0
        || number(&attempt["total"])? != number(&attempt["emitted"])?
        || number(&attempt["emitted"])? != number(&attempt["observed"])?
    {
        return Err("Current attempt lacks complete, untainted observed error coverage".into());
    }
    Ok(scope.clone())
}
