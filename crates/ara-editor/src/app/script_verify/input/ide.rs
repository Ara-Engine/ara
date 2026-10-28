//! Validate the retained literal invocation while the production launch is suppressed.
use super::*;
impl ScriptCheck {
    pub(super) fn ide_location(
        &self,
        editor: &Editor,
        shell: &Shell,
        line: u64,
        column: u64,
    ) -> ara_ui::Result<Value> {
        let source = self
            .source
            .as_ref()
            .ok_or("J10 source missing")?
            .canonicalize()?;
        let source = source.to_str().ok_or("J10 source is not absolute UTF-8")?;
        let root = editor.host.root().canonicalize()?;
        let root = root
            .to_str()
            .ok_or("J10 project root is not absolute UTF-8")?;
        let request = shell.script_ide_request();
        let program = if cfg!(windows) { "code.cmd" } else { "code" };
        let args = json!([
            "--reuse-window",
            "--goto",
            format!("{source}:{line}:{column}")
        ]);
        if request["targetFile"] != source
            || request["line"] != line
            || request["column"] != column
            || request["cwd"] != root
            || request["program"] != program
            || request["args"] != args
            || request["lineHandling"] != "vscode-goto"
            || request["launchSuppressed"] != true
            || request["attempted"] != false
            || request["outcome"] != "suppressed"
            || !request["error"].is_null()
        {
            return Err(format!("Native compiler Go retained the wrong IDE invocation: {request}; expected target {source}, argv {args}").into());
        }
        if let Some(fallback) = request
            .get("fallback")
            .filter(|fallback| !fallback.is_null())
            && (fallback["launchSuppressed"] != true
                || fallback["attempted"] != false
                || fallback["outcome"] != "suppressed")
        {
            return Err("Native compiler Go attempted its fallback despite offscreen policy".into());
        }
        Ok(request)
    }
}
