//! Development resources are read on a worker; the UI only receives complete,
//! validated snapshots. Release builds use the embedded resources exclusively.
use super::*;
#[cfg(any(test, not(debug_assertions)))]
use ara_ui::theme::EditorTheme;
#[cfg(debug_assertions)]
use ara_ui::theme::tokens_from_json;

type Tokens = BTreeMap<String, String>;
#[cfg(debug_assertions)]
type Updates = std::sync::mpsc::Receiver<Result<Snapshot, String>>;
#[cfg(debug_assertions)]
use ara_ui::live::Stop;

/// Last good resources, shared by all retained editor surfaces.
pub(super) struct LiveStyles {
    css: BTreeMap<String, String>,
    tokens: [Tokens; 2],
    #[cfg(debug_assertions)]
    updates: Updates,
    #[cfg(debug_assertions)]
    _stop: Stop,
}

#[cfg(debug_assertions)]
#[derive(Debug)]
struct Snapshot {
    css: BTreeMap<String, String>,
    tokens: [Tokens; 2],
}

/// The shell stylesheets. Release builds compile them in; debug builds compile none
/// (a CSS edit never recompiles the editor) and `LiveStyles` reads the checkout
/// before the first frame and after every save instead.
pub(super) mod embedded {
    macro_rules! sheet {
        ($name:ident, $path:literal) => {
            #[cfg(not(debug_assertions))]
            pub(in crate::shell) const $name: &str = include_str!($path);
            #[cfg(debug_assertions)]
            pub(in crate::shell) const $name: &str = "";
        };
    }
    sheet!(EDITOR, "editor.css");
    sheet!(VIEWPORT, "viewport.css");
    sheet!(INSPECTOR, "inspector.css");
    sheet!(COMFORTABLE, "editor-comfortable.css");
    sheet!(TITLEBAR, "editor-titlebar.css");
    sheet!(HISTORY, "history_panel/history.css");
    sheet!(LIGHTING, "lighting/lighting.css");
    sheet!(PROBES, "probes/probes.css");
}

impl LiveStyles {
    pub(super) fn new() -> ara_ui::Result<Self> {
        #[cfg(debug_assertions)]
        let root = std::env::var_os("ARA_EDITOR_STYLE_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
        // Debug builds embed no shell CSS, so the first snapshot is read now.
        #[cfg(debug_assertions)]
        let first = read_snapshot(&root).map_err(|e| format!("editor styles: {e}"))?;
        #[cfg(debug_assertions)]
        let (updates, _stop) = watch(root)?;
        Ok(Self {
            #[cfg(debug_assertions)]
            css: first.css,
            #[cfg(debug_assertions)]
            tokens: first.tokens,
            #[cfg(not(debug_assertions))]
            css: BTreeMap::new(),
            #[cfg(not(debug_assertions))]
            tokens: [EditorTheme::Dark.tokens()?, EditorTheme::Light.tokens()?],
            #[cfg(debug_assertions)]
            updates,
            #[cfg(debug_assertions)]
            _stop,
        })
    }

    pub(super) fn css<'a>(&'a self, name: &str, embedded: &'a str) -> &'a str {
        self.css.get(name).map(String::as_str).unwrap_or(embedded)
    }

    pub(super) fn tokens(&self, light: bool) -> Tokens {
        self.tokens[usize::from(light)].clone()
    }

    fn poll(&mut self) -> Result<bool, String> {
        #[cfg(debug_assertions)]
        {
            let Some(update) = self.updates.try_iter().last() else {
                return Ok(false);
            };
            let snapshot = update?;
            let changed = self.css != snapshot.css || self.tokens != snapshot.tokens;
            self.css = snapshot.css;
            self.tokens = snapshot.tokens;
            Ok(changed)
        }
        #[cfg(not(debug_assertions))]
        {
            Ok(false)
        }
    }
}

impl Shell {
    /// Poll the resource worker without filesystem work on the UI thread. Invalid
    /// saves retain the last good styles and report once per changed resource set.
    /// Returns (styles changed, viewport extent changed).
    pub(crate) fn reload_styles(&mut self, editor: &mut Editor) -> ara_ui::Result<(bool, bool)> {
        match self.styles.poll() {
            Ok(true) => {
                let before = self.viewport().map(|r| [r.w, r.h]);
                self.rebuild(editor)?;
                Ok((true, before != self.viewport().map(|r| [r.w, r.h])))
            }
            Ok(false) => Ok((false, false)),
            Err(error) => {
                self.pending_reports.push(("editor styles", error));
                Ok((false, false))
            }
        }
    }
}

/// Both themes, every CSS file under the shell (panel subfolders included) and the
/// shared ara-ui dialog/select CSS, sorted. Snapshots key CSS by file name, so those
/// names must stay unique; `read_snapshot` rejects a duplicate.
#[cfg(debug_assertions)]
fn resource_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut paths = vec![
        root.join("assets/themes/ara-dark.json"),
        root.join("assets/themes/ara-light.json"),
    ];
    paths.extend(
        ["dialogs/dialogs.css", "select/select.css"]
            .map(|file| root.join("crates/ara-ui/src").join(file))
            .into_iter()
            .filter(|path| path.is_file()),
    );
    let mut directories = vec![root.join("crates/ara-editor/src/shell")];
    while let Some(directory) = directories.pop() {
        for path in std::fs::read_dir(directory)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
        {
            if path.is_dir() {
                directories.push(path);
            } else if path.extension().is_some_and(|e| e == "css") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

#[cfg(debug_assertions)]
fn read_snapshot(root: &std::path::Path) -> Result<Snapshot, String> {
    let read = |path: &std::path::Path| {
        std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
    };
    let parse = |name: &str| {
        let source = read(&root.join("assets/themes").join(name))?;
        ara_json::parse(&source).map_err(|e| format!("{name}: {e}"))?;
        tokens_from_json(&source).map_err(|e| format!("{name}: {e}"))
    };
    let tokens = [parse("ara-dark.json")?, parse("ara-light.json")?];
    let mut css = BTreeMap::new();
    for path in resource_files(root) {
        if path.extension().is_some_and(|e| e == "css") {
            let source = read(&path)?;
            ara_ui::theme::check_editor("<ui id='style-validation' />", &source)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if let Some(name) = path.file_name() {
                let name = name.to_string_lossy().into_owned();
                if css.insert(name.clone(), source).is_some() {
                    return Err(format!("two live stylesheets are named {name}"));
                }
            }
        }
    }
    // Resolve both densities and palettes before publishing anything to the UI.
    for theme in &tokens {
        let mut source = css.get("editor.css").ok_or("missing editor.css")?.clone();
        for (name, extra) in &css {
            if name != "editor.css" && name != "editor-comfortable.css" {
                source.push_str(extra);
            }
        }
        ara_ui::style::Sheet::with_tokens(&source, theme.clone()).map_err(|e| e.to_string())?;
        if let Some(comfortable) = css.get("editor-comfortable.css") {
            source.push_str(comfortable);
        }
        ara_ui::style::Sheet::with_tokens(&source, theme.clone()).map_err(|e| e.to_string())?;
    }
    Ok(Snapshot { css, tokens })
}

/// The editor loop polls every frame, so the worker needs no wake.
#[cfg(debug_assertions)]
fn watch(root: std::path::PathBuf) -> std::io::Result<(Updates, Stop)> {
    let files = root.clone();
    ara_ui::live::watch(
        "editor-styles",
        move || resource_files(&files),
        move || read_snapshot(&root),
        || {},
    )
}

#[cfg(all(test, debug_assertions))]
#[path = "live_styles_tests.rs"]
mod tests;
