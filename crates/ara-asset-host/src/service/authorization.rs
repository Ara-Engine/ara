//! Validate every capability and typed argument before any asset worker can read.
use ara_diag::{Diagnostic, codes};
use ara_host::{Cx, params::ApplyParams};
use serde_json::json;
pub(super) fn authorize(cx: &Cx<'_>, params: &ApplyParams) -> Result<(), Vec<Diagnostic>> {
    let profiles = params
        .commands
        .iter()
        .any(|call| call.cmd == "build_profile.create");
    if profiles
        && params
            .commands
            .iter()
            .any(|call| call.cmd != "build_profile.create")
    {
        return Err(vec![Diagnostic::error(
            codes::INVALID_PARAMS,
            "build profile creation is a project-settings batch; submit asset operations separately",
        )]);
    }
    if let Some((index, call)) = params.commands.iter().enumerate().find(|(_, call)| {
        !call.cmd.starts_with("asset.")
            && !matches!(call.cmd.as_str(), "folder.create" | "build_profile.create")
    }) {
        return Err(vec![Diagnostic::error(codes::INVALID_PARAMS, format!("asset batches accept asset operations and folder.create; submit `{}` in its own typed batch", call.cmd)).command(index)]);
    }
    if params.commands.len() > 1
        && params
            .commands
            .iter()
            .any(|call| call.cmd == "asset.import")
    {
        return Err(vec![Diagnostic::error(
            codes::INVALID_PARAMS,
            "asset.import is one atomic job; put the full selection in sources and submit other asset operations as a separate batch",
        )]);
    }
    let session = cx.session().ok_or_else(|| {
        vec![Diagnostic::error(
            codes::SESSION_NOT_FOUND,
            "asset preparation needs a live session",
        )]
    })?;
    for (index, call) in params.commands.iter().enumerate() {
        let info = cx.registry().info(&call.cmd).ok_or_else(|| {
            vec![
                Diagnostic::error(
                    codes::UNKNOWN_COMMAND,
                    format!("unknown command `{}`", call.cmd),
                )
                .command(index),
            ]
        })?;
        let args = call.args.clone().unwrap_or_else(|| json!({}));
        let object = args.as_object().ok_or_else(|| {
            vec![
                Diagnostic::error(codes::INVALID_PARAMS, "command arguments must be an object")
                    .command(index),
            ]
        })?;
        let desc = cx.store().schema().get(&info.params).ok_or_else(|| {
            vec![
                Diagnostic::error(
                    codes::INVALID_PARAMS,
                    "command argument schema is unavailable",
                )
                .command(index),
            ]
        })?;
        let mut problems = Vec::new();
        ara_schema::Validator::new(cx.store().schema(), &mut problems)
            .allow_symbols()
            .fields(desc, object, "args");
        let problems: Vec<_> = problems
            .into_iter()
            .filter(Diagnostic::is_error)
            .map(|diagnostic| diagnostic.command(index))
            .collect();
        if !problems.is_empty() {
            return Err(problems);
        }
        let mut required = vec![info.scope];
        if call.cmd.starts_with("asset.")
            || matches!(call.cmd.as_str(), "folder.create" | "build_profile.create")
        {
            required.push("edit:assets");
        }
        if call.cmd == "asset.import" {
            required.extend(["import", "fs:outside-project"]);
        }
        for scope in required {
            if !session
                .granted
                .iter()
                .any(|granted| granted.as_str() == scope)
            {
                return Err(vec![
                    Diagnostic::error(
                        codes::MISSING_SCOPE,
                        format!(
                            "`{}` needs `{scope}` before asset preparation can read files",
                            call.cmd
                        ),
                    )
                    .command(index),
                ]);
            }
        }
    }
    if params.branch.is_some() {
        return Err(vec![Diagnostic::error(
            codes::INVALID_PARAMS,
            "asset file operations must commit to the live project; branch file preparation is unavailable",
        )]);
    }
    Ok(())
}
