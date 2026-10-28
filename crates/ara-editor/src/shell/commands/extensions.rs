//! Both native and C# registrations flow through this registry view.
use super::*;
use crate::extensions::{Catalog, Command};

#[derive(Clone, Copy)]
pub(crate) struct Entry<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub summary: &'a str,
    pub icon: &'static str,
    pub keys: &'static str,
    pub builtin: Option<&'static Spec>,
    enabled: bool,
    disabled_reason: Option<&'a str>,
}
impl Entry<'_> {
    pub fn title(&self, cx: &Ctx) -> String {
        self.builtin
            .map_or_else(|| self.title.into(), |s| s.title(cx))
    }
    pub fn avail(&self, cx: &Ctx) -> Avail {
        self.builtin.map_or_else(
            || {
                if self.enabled {
                    Avail::Ready
                } else {
                    Avail::Blocked(
                        self.disabled_reason
                            .unwrap_or("C# editor command is disabled until reload")
                            .into(),
                    )
                }
            },
            |s| s.avail(cx),
        )
    }
    pub fn binding(&self, target: Target) -> super::super::Binding {
        match self.builtin {
            Some(spec) => super::super::Binding::Command(spec.id, target),
            None => super::super::Binding::CommandOwned(self.id.into(), target),
        }
    }
}
fn native(spec: &'static Spec) -> Entry<'static> {
    Entry {
        id: spec.id,
        title: spec.title,
        summary: spec.summary,
        icon: spec.icon,
        keys: spec.keys,
        builtin: Some(spec),
        enabled: true,
        disabled_reason: None,
    }
}
fn extension(spec: &Command) -> Entry<'_> {
    Entry {
        id: &spec.id,
        title: &spec.title,
        summary: &spec.summary,
        icon: "command",
        keys: "",
        builtin: None,
        enabled: spec.enabled,
        disabled_reason: spec.disabled_reason.as_deref(),
    }
}
pub(crate) fn entries(catalog: &Catalog) -> impl Iterator<Item = Entry<'_>> + '_ {
    let builtin = all().iter().map(|spec| -> Entry<'_> { native(spec) });
    builtin.chain(catalog.commands.iter().map(extension))
}
pub(crate) fn lookup<'a>(catalog: &'a Catalog, id: &str) -> Option<Entry<'a>> {
    get(id)
        .map(native)
        .or_else(|| catalog.commands.iter().find(|c| c.id == id).map(extension))
}
pub(crate) fn registered_menu_path(catalog: &Catalog, id: &str) -> String {
    catalog
        .menus
        .iter()
        .find(|m| m.command == id)
        .map(|m| {
            m.path
                .rsplit_once('/')
                .map_or("", |(p, _)| p)
                .replace('/', " › ")
        })
        .unwrap_or_else(|| menu_path(id))
}
pub(crate) fn bar_titles(catalog: &Catalog) -> Vec<&str> {
    let mut titles: Vec<_> = menus::MENUS.iter().map(|m| m.title).collect();
    for menu in &catalog.menus {
        if let Some((root, _)) = menu.path.split_once('/')
            && !titles.contains(&root)
        {
            titles.push(root);
        }
    }
    titles
}
