//! Catalogued failures retain their fixes, logs and retry actions.
use super::*;
use ara_diag::FixAction;

pub(super) fn body(p: &mut Page, hub: &Hub, id: u64) {
    let Some(notice) = hub.notices.iter().find(|notice| notice.id == id) else {
        return;
    };
    heading(p, "circle-alert", &notice.title);
    p.markup.push_str("<scroll id='dialog-error-scroll' class='dialog-error-scroll'><panel id='dialog-error-details' class='dialog-error-details'>");
    let mut fix_index = 0;
    for diagnostic in &notice.diagnostics {
        p.text("dialog-copy", &diagnostic.message);
        p.text("small muted", &diagnostic.code.to_string());
        for fix in &diagnostic.fixes {
            let index = fix_index;
            fix_index += 1;
            match &fix.action {
                FixAction::Call { method, .. } if method.starts_with("hub.") => p.action(
                    &format!("dialog-fix-{index}"),
                    "compact",
                    "check",
                    &fix.title,
                    Some(Intent::Fix {
                        notice: id,
                        fix: index,
                    }),
                ),
                FixAction::Cli { args } => {
                    p.text("dialog-copy muted", &fix.title);
                    p.copy(
                        hub.copied.as_deref(),
                        &format!("dialog-copy-fix-{index}"),
                        &format!("ara {}", args.join(" ")),
                        false,
                    );
                }
                _ => p.text("dialog-copy muted", &fix.title),
            }
        }
    }
    if let Some(detail) = &notice.detail {
        p.text("dialog-copy muted", detail);
    }
    p.markup.push_str("</panel></scroll>");
    p.panel("dialog-actions", |p| {
        if let Some(retry) = &notice.retry {
            p.action(
                "dialog-retry-operation",
                "",
                "refresh-cw",
                "Retry",
                Some((**retry).clone()),
            );
        }
        if let Some(log) = &notice.log {
            p.action(
                "dialog-open-log",
                "",
                "file",
                "Open log",
                Some(Intent::OpenPath(log.clone())),
            );
        }
        if notice.retry.is_none()
            && let Some(job) = hub.releases.jobs.iter().find(|job| {
                job.state == "failed"
                    && job
                        .error
                        .as_ref()
                        .is_some_and(|error| notice.diagnostics.contains(error))
            })
        {
            let retry = if job.kind == "uninstall" {
                Intent::RetryUninstall(job.version.clone())
            } else {
                Intent::Dialog(if job.kind == "hub" {
                    Dialog::Update
                } else if job.kind == "repair" {
                    Dialog::RepairEngine(job.version.clone())
                } else {
                    Dialog::InstallEngine(job.version.clone())
                })
            };
            p.action("dialog-retry", "", "refresh-cw", "Retry", Some(retry));
        }
        p.leaf("fill");
        p.action(
            "dialog-error-dismiss",
            "primary",
            "",
            "Got it",
            Some(Intent::Dismiss(id)),
        );
    });
}
