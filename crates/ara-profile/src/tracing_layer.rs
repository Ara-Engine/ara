#[cfg(not(feature = "tracy"))]
use crate::ring::{Name, PRODUCER};
use std::cell::RefCell;
use tracing::{
    Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id},
};
use tracing_subscriber::{Layer, layer::Context, registry::LookupSpan};

#[cfg(not(feature = "tracy"))]
type EnteredScope = Option<(u64, u64)>;
#[cfg(feature = "tracy")]
type EnteredScope = Option<tracy_client::Span>;
struct Entered {
    entries: [Option<(u64, EnteredScope)>; 128],
    len: usize,
    overflow: usize,
}
thread_local! {
    static ENTERED: RefCell<Entered> = const { RefCell::new(Entered {
        entries: [const { None }; 128], len: 0, overflow: 0,
    }) };
}
struct Label {
    name: String,
    #[cfg(not(feature = "tracy"))]
    owner: u64,
    #[cfg(not(feature = "tracy"))]
    index: u32,
}
struct NameVisitor(Option<String>);
impl Visit for NameVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.0 = Some(value.into());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "name" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}
/// Composable tracing adapter, including Bevy's dynamic system `name` field.
/// Attach a profiler thread endpoint before entering spans. Each synchronous
/// enter/exit excludes async suspension. Dynamic names are interned once per
/// tracing span/capture. With Tracy, one static adapter location carries aliases
/// as zone text; `scope!` literal sites use their own cached source locations.
#[derive(Debug, Default)]
pub struct ProfileLayer;
impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for ProfileLayer {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, cx: Context<'_, S>) {
        let Some(span) = cx.span(id) else {
            return;
        };
        let mut visitor = NameVisitor(None);
        attrs.record(&mut visitor);
        span.extensions_mut().insert(Label {
            name: visitor.0.unwrap_or_else(|| attrs.metadata().name().into()),
            #[cfg(not(feature = "tracy"))]
            owner: 0,
            #[cfg(not(feature = "tracy"))]
            index: 0,
        });
    }
    fn on_enter(&self, id: &Id, cx: Context<'_, S>) {
        let Some(span) = cx.span(id) else {
            return;
        };
        ENTERED.with_borrow_mut(|entered| {
            if entered.len == entered.entries.len() {
                entered.overflow += 1;
                crate::ring::mark_dropped();
                return;
            }
            #[cfg(not(feature = "tracy"))]
            let scope = if crate::instrumentation_enabled() {
                let mut extensions = span.extensions_mut();
                let Some(label) = extensions.get_mut::<Label>() else {
                    return;
                };
                let name = PRODUCER.with_borrow(|p| {
                    let p = p.as_ref()?;
                    if label.owner != p.control.id {
                        let mut names = p
                            .control
                            .names
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        label.index = if let Some(index) = names.ids.get(&label.name) {
                            *index
                        } else {
                            let index = names.values.len() as u32;
                            names.values.push(label.name.to_owned());
                            names.ids.insert(label.name.to_owned(), index);
                            index
                        };
                        label.owner = p.control.id;
                    }
                    Some(Name::Registered(label.index))
                });
                name.and_then(|name| crate::ring::begin(name, &label.name))
            } else {
                None
            };
            #[cfg(feature = "tracy")]
            let scope = if crate::instrumentation_enabled() {
                let extensions = span.extensions();
                let Some(label) = extensions.get::<Label>() else {
                    return;
                };
                tracy_client::Client::running().map(|client| {
                    let zone = client.span(tracy_client::span_location!("Ara tracing"), 0);
                    zone.emit_text(&label.name);
                    zone
                })
            } else {
                None
            };
            entered.entries[entered.len] = Some((id.into_u64(), scope));
            entered.len += 1;
        });
    }
    fn on_exit(&self, id: &Id, _: Context<'_, S>) {
        ENTERED.with_borrow_mut(|entered| {
            if entered.overflow > 0 {
                entered.overflow -= 1;
                return;
            }
            if let Some(i) = entered.entries[..entered.len]
                .iter()
                .rposition(|e| e.as_ref().is_some_and(|e| e.0 == id.into_u64()))
            {
                if let Some((_id, scope)) = entered.entries[i].take() {
                    #[cfg(not(feature = "tracy"))]
                    if let Some((owner, scope)) = scope {
                        crate::ring::end(owner, scope);
                    }
                    #[cfg(feature = "tracy")]
                    drop(scope);
                }
                while entered.len > 0 && entered.entries[entered.len - 1].is_none() {
                    entered.len -= 1;
                }
            }
        });
    }
}
