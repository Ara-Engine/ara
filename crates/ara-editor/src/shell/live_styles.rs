//! Development resources are read on a worker; the UI only receives complete,
//! validated snapshots. Release builds use the embedded resources exclusively.
use super::*;
use ara_ui::theme::EditorTheme;
#[cfg(debug_assertions)]
use ara_ui::theme::tokens_from_json;

type Tokens = BTreeMap<String, String>;
#[cfg(debug_assertions)]
type Updates = std::sync::mpsc::Receiver<Result<Snapshot, String>>;
#[cfg(debug_assertions)]
type Stop = std::sync::mpsc::Sender<()>;

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

impl LiveStyles {
    pub(super) fn new() -> ara_ui::Result<Self> {
        #[cfg(debug_assertions)]
        let (updates, _stop) = watch(
            std::env::var_os("ARA_EDITOR_STYLE_ROOT")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
                }),
        )?;
        Ok(Self {
            css: BTreeMap::new(),
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

#[cfg(debug_assertions)]
fn resource_files(
    root: &std::path::Path,
) -> Vec<(std::path::PathBuf, Option<std::time::SystemTime>, u64)> {
    let mut paths = vec![
        root.join("assets/themes/ara-dark.json"),
        root.join("assets/themes/ara-light.json"),
    ];
    if let Ok(files) = std::fs::read_dir(root.join("crates/ara-editor/src/shell")) {
        paths.extend(
            files
                .flatten()
                .map(|f| f.path())
                .filter(|p| p.extension().is_some_and(|e| e == "css")),
        );
    }
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let metadata = p.metadata().ok();
            let modified = metadata.as_ref().and_then(|m| m.modified().ok());
            let len = metadata.map_or(0, |m| m.len());
            (p, modified, len)
        })
        .collect()
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
    for (path, _, _) in resource_files(root) {
        if path.extension().is_some_and(|e| e == "css") {
            let source = read(&path)?;
            ara_ui::theme::check_editor("<ui id='style-validation' />", &source)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if let Some(name) = path.file_name() {
                css.insert(name.to_string_lossy().into_owned(), source);
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

#[cfg(debug_assertions)]
fn watch(root: std::path::PathBuf) -> std::io::Result<(Updates, Stop)> {
    let (tx, updates) = std::sync::mpsc::channel();
    let (stop, stopped) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("editor-styles".into())
        .spawn(move || {
            let mut fingerprint = Vec::new();
            loop {
                let next = resource_files(&root);
                if next != fingerprint {
                    fingerprint = next;
                    if tx.send(read_snapshot(&root)).is_err() {
                        break;
                    }
                }
                match stopped.recv_timeout(std::time::Duration::from_millis(250)) {
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    _ => break,
                }
            }
        })?;
    Ok((updates, stop))
}

#[cfg(all(test, debug_assertions))]
#[path = "live_styles_tests.rs"]
mod tests;
