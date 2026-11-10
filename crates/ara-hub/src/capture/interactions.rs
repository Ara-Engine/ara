//! Interaction captures: real input through the runtime (the project menu, inline rename,
//! search, the command palette, a dropdown), each checked against the model it drives.
use super::*;

pub(super) fn run(cap: &mut Capture, folder: &Path, light: bool) -> anyhow::Result<()> {
    let theme = if light { "light" } else { "dark" };
    let shot = |name: &str| folder.join(format!("{theme}-{name}.png"));
    let mut hub = Hub::new(
        crate::model::Prefs {
            light,
            ..Default::default()
        },
        "target/hub-flows".into(),
        None,
    );
    seed(&mut hub);
    hub.view = View::Projects;
    cap.click(&mut hub, "more-project-1")?;
    anyhow::ensure!(
        hub.menu.as_deref() == Some("project-1"),
        "⋯ opens the project's menu"
    );
    cap.save(&mut hub, &shot("projects-menu"))?;
    cap.click(&mut hub, "menu-rename")?;
    anyhow::ensure!(
        hub.menu.is_none() && hub.rename.as_ref().is_some_and(|(id, _)| id == "project-1"),
        "Rename closes the menu and edits in place"
    );
    cap.save(&mut hub, &shot("projects-rename"))?;
    hub.intent(Intent::RenameCancel);
    hub.intent(Intent::Search("night".into()));
    cap.save(&mut hub, &shot("projects-search"))?;
    hub.intent(Intent::Search("zzz".into()));
    cap.save(&mut hub, &shot("projects-no-match"))?;
    hub.intent(Intent::Search(String::new()));
    cap.window(&mut hub, |s| {
        s.palette.open();
        s.palette.query = "o".into();
    });
    cap.save(&mut hub, &shot("palette"))?;
    cap.window(&mut hub, |s| s.palette.open = false);
    hub.intent(Intent::Navigate(View::Create));
    cap.click(&mut hub, "template-outpost")
        .or_else(|_| cap.click(&mut hub, "template-starter"))?;
    cap.click(&mut hub, "create-engine")?;
    cap.save(&mut hub, &shot("create-engine-select"))?;
    cap.window(&mut hub, |s| s.select = None);
    Ok(())
}
