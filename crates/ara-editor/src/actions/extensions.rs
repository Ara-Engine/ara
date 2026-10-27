//! Completion runs on the authority; callbacks only return typed commands and navigation.
use super::*;
use crate::extensions::{Completion, Pending};

impl EditorService {
    pub(super) fn extension_command(&mut self, cx: &mut Cx<'_>, command: EditorCommand) -> R {
        let session = command.session;
        let state = &self.navigation.state;
        let agent = cx
            .session()
            .is_some_and(|s| matches!(s.author, ara_ops::Author::Agent(_)));
        let presence = agent.then(|| state.presence.get(&session)).flatten();
        let doc = presence.map_or(&state.doc, |p| &p.doc).clone();
        let mut selection = if agent {
            presence.map_or_else(Vec::new, |p| p.selection.clone())
        } else {
            state.selection.clone()
        };
        if let Some(target) = command
            .target
            .as_ref()
            .and_then(|v| v.get("entity"))
            .and_then(Value::as_str)
        {
            selection = vec![target.to_owned()];
        }
        let resolved = self.resolver.resolve(cx.store(), &doc)?;
        let entities: Vec<_> = resolved.iter().map(|(id, entity)| {
            let record = &entity.record;
            let components: serde_json::Map<_, _> = record.iter()
                .filter(|(key, _)| ara_doc::is_component_key(key)).map(|(key, value)| (key.clone(), value.clone())).collect();
            json!({"id":id, "name":record.get("name").and_then(Value::as_str).unwrap_or(id),
                "parent":entity.parent, "order":entity.order.as_deref().unwrap_or(""), "components":components})
        }).collect();
        let request = json!({"operation":"invoke", "command":command.id,
            "generation":self.extensions.catalog.generation, "scene":{"entities":entities}, "selection":selection});
        let job = cx.start_job(&format!("C# editor command {}", command.id));
        self.extensions
            .invoke(
                job.clone(),
                Pending {
                    session,
                    doc,
                    revision: cx.store().revision(),
                    command: command.id,
                },
                request,
            )
            .map_err(|d| vec![d])?;
        Ok(json!({"ok":true,"job":job,"preparing":true}))
    }

    pub(super) fn tick_extensions(&mut self, cx: &mut Cx<'_>) {
        while let Ok(completion) = self.extensions.completed.try_recv() {
            match completion {
                Completion::Catalog(Ok(reply)) => {
                    for diagnostic in reply.diagnostics {
                        cx.log(diagnostic);
                    }
                    if reply.accepted && reply.catalog != self.extensions.catalog {
                        self.extensions.catalog = reply.catalog;
                        let _ = self
                            .controls
                            .send(Control::Extensions(self.extensions.catalog.clone()));
                    }
                }
                Completion::Catalog(Err(diagnostic)) => cx.log(diagnostic),
                Completion::Invoke(job, reply) => {
                    let Some(pending) = self.extensions.pending.remove(&job) else {
                        continue;
                    };
                    if cx.job_cancelled(&job) {
                        continue;
                    }
                    let result = (|| {
                        let reply = reply.map_err(|d| vec![d])?;
                        if reply.catalog.generation == self.extensions.catalog.generation
                            && reply.catalog != self.extensions.catalog
                        {
                            self.extensions.catalog = reply.catalog.clone();
                            let _ = self
                                .controls
                                .send(Control::Extensions(reply.catalog.clone()));
                        }
                        if !reply.accepted {
                            return Err(reply.diagnostics);
                        }
                        if cx.store().revision() != pending.revision {
                            return Err(vec![Diagnostic::error(
                                ara_diag::codes::STALE_BASE,
                                "scene changed while the C# editor callback ran; invoke it again",
                            )]);
                        }
                        cx.resume_session(&pending.session, |cx| {
                            let mut output = if reply.authoring.is_empty() {
                                json!({"ok":true,"ids":{},"ops":[]})
                            } else {
                                cx.apply(ara_host::params::ApplyParams {
                                    session:pending.session.clone(), doc:Some(pending.doc.clone()), commands:reply.authoring,
                                    why:Some(format!("Run C# editor command {}", pending.command)), base:Some(pending.revision),
                                    dry_run:None, idempotency_key:None, branch:None,
                                })?
                            };
                            if let Some(selection) = reply.selection {
                                let selection: Vec<_> = selection.into_iter().map(|id| output["ids"][&id].as_str().map(str::to_owned).unwrap_or(id)).collect();
                                // Reuse D24's private agent presence and navigation ops.
                                output["navigation"] = self.dispatch(cx, "editor.select", json!({"session":pending.session,"doc":pending.doc,"entities":selection}))?;
                            }
                            Ok(output)
                        })?
                    })();
                    if let Err(diagnostics) = &result {
                        for diagnostic in diagnostics {
                            cx.log(diagnostic.clone());
                        }
                    }
                    cx.finish_job(&job, result);
                }
            }
        }
    }
}
