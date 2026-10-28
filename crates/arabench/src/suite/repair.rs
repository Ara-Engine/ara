//! Repairs: broken `@path#id` references, an `ara merge` conflict, diagnostics with
//! fixes.

use std::fs;
use std::path::Path;

use ara_host::Project;
use serde_json::{Value, json};

use super::{OUTPOST, at, clean, num_is, vec_is};
use crate::fixture::{authored_path, patch};
use crate::solve::{Solver, git};
use crate::task::{Area, Budget, Checks, Fixture, Task};
use crate::world::World;

const YARD_LAMP: &str = "\"name\": \"YardLamp\",\n      \"order\": \"aD\",\n      \"prefab\": \"@prefabs/lamp.prefab#pf_lamp0001\"";
const GENERATOR_MESH: &str = "\"@meshes/generator.glb#ms_genr0001\"";

/// Both halves of two references broken, as after an asset rename.
pub const BROKEN_REFS: Task = Task {
    id: "refs-fix-broken",
    area: Area::References,
    prompt: "`ara check` fails on the outpost level since some assets were renamed. Fix the broken references so it passes again.",
    fixture: Fixture::Outpost,
    setup: Some(|root: &Path| {
        patch(
            root,
            OUTPOST,
            YARD_LAMP,
            &YARD_LAMP.replace("lamp.prefab#pf_lamp0001", "lantern.prefab#pf_lantrn01"),
        )?;
        patch(
            root,
            OUTPOST,
            GENERATOR_MESH,
            "\"@meshes/genrator.glb#ms_genr0002\"",
        )
    }),
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        clean(c, w);
        let prefab = w
            .record(OUTPOST, "e_ylamp1")
            .and_then(|r| r.get("prefab"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        c.check(
            "YardLamp is an instance of the lamp prefab again",
            prefab.contains("pf_lamp0001"),
            format!("prefab {prefab:?}"),
        );
        if let Some(e) = at(c, w, OUTPOST, "e_ylamp1") {
            vec_is(
                c,
                "YardLamp keeps its position",
                Some(e.position()),
                &[0.0, 0.0, 8.0],
            );
        }
        if let Some(e) = w.get(OUTPOST, "e_ylamp1/e_bulb01") {
            vec_is(
                c,
                "YardLamp keeps its red bulb",
                e.vec("Light.color"),
                &[1.0, 0.0, 0.0, 1.0],
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_gen001") {
            let mesh = e.get("MeshRenderer.mesh");
            c.check(
                "Generator mesh is the generator mesh",
                mesh == Some(json!("@meshes/generator.glb#ms_genr0001")),
                format!("{mesh:?}"),
            );
        }
    },
    solve: |s: &mut Solver| {
        s.apply(
            OUTPOST,
            "Point the generator at its renamed mesh",
            json!([{ "cmd": "property.set", "args": { "entity": "e_gen001", "path": "MeshRenderer.mesh", "value": "@meshes/generator.glb#ms_genr0001" } }]),
        )?;
        s.flush();
        let path = authored_path(s.root(), OUTPOST);
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        s.write_authored(
            OUTPOST,
            &text.replace(
                "@prefabs/lantern.prefab#pf_lantrn01",
                "@prefabs/lamp.prefab#pf_lamp0001",
            ),
        )
    },
};

/// Creates a real `git merge` conflict on the outpost level: `ara merge` output
/// with one `$conflicts` entry (the barrel's name) and git mid-merge.
fn conflicted_merge(root: &Path) -> Result<(), String> {
    let g = |args: &[&str]| git(root, args);
    for (file, line) in [
        (".gitignore", ".ara/"),
        (".gitattributes", "*.scene merge=ara"),
    ] {
        let path = root.join(file);
        let text = fs::read_to_string(&path).unwrap_or_default();
        if !text.lines().any(|l| l.trim() == line) {
            fs::write(&path, format!("{text}{line}\n")).map_err(|e| e.to_string())?;
        }
    }
    g(&["init", "-q", "-b", "main"])?;
    g(&["config", "user.name", "Level Designer"])?;
    g(&["config", "user.email", "designer@example.com"])?;
    g(&["config", "commit.gpgsign", "false"])?;
    g(&["config", "core.autocrlf", "false"])?;
    g(&["add", "-A"])?;
    g(&["commit", "-q", "-m", "Outpost level"])?;
    g(&["checkout", "-q", "-b", "teammate"])?;
    patch(
        root,
        OUTPOST,
        "\"name\": \"Barrel\",",
        "\"name\": \"Oil Barrel\",",
    )?;
    patch(root, OUTPOST, "\"intensity\": 0,", "\"intensity\": 1500,")?;
    g(&["commit", "-q", "-am", "Oil barrel; floodlight on"])?;
    g(&["checkout", "-q", "main"])?;
    patch(
        root,
        OUTPOST,
        "\"name\": \"Barrel\",",
        "\"name\": \"Fuel Barrel\",",
    )?;
    patch(
        root,
        OUTPOST,
        "\"position\": [6, 0, -4]",
        "\"position\": [6, 0, -5]",
    )?;
    g(&["commit", "-q", "-am", "Fuel barrel; generator moved"])?;
    // A stand-in driver that keeps ours and reports a conflict; the in-process merge
    // then writes exactly what `ara merge` produces.
    g(&["config", "merge.ara.driver", "false"])?;
    if g(&["merge", "-q", "--no-edit", "teammate"]).is_ok() {
        return Err("the merge unexpectedly succeeded".into());
    }
    let db = Project::open(root).map_err(|d| d.to_string())?.db;
    let stage = |n: u8| -> Result<ara_doc::Document, String> {
        let text = g(&[
            "show",
            &format!(":{n}:{}/{OUTPOST}", ara_doc::AUTHORING_ROOT),
        ])?;
        ara_doc::read(&text, OUTPOST, &db, None)
            .doc
            .ok_or_else(|| format!("stage {n} does not read"))
    };
    let merged =
        ara_merge::merge(&stage(1)?, &stage(2)?, &stage(3)?, &db).map_err(|d| d.to_string())?;
    fs::write(
        authored_path(root, OUTPOST),
        ara_doc::write(&merged.doc, &db),
    )
    .map_err(|e| e.to_string())?;
    g(&["config", "merge.ara.name", "Ara semantic merge"])?;
    g(&[
        "config",
        "merge.ara.driver",
        "ara --root \".\" merge %O %A %B",
    ])?;
    Ok(())
}

/// Resolving the conflict `ara merge` left and concluding the git merge.
pub const MERGE_CONFLICT: Task = Task {
    id: "merge-resolve-conflict",
    area: Area::Merge,
    prompt: "I merged the `teammate` branch into main and git stopped with a conflict in content/levels/outpost.scene. Resolve it (for the barrel's name, keep mine) and finish the merge.",
    fixture: Fixture::Outpost,
    setup: Some(conflicted_merge),
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        clean(c, w);
        let text = w.authored_file(OUTPOST).unwrap_or_default();
        c.check(
            "no `$conflicts` left",
            !text.contains("$conflicts"),
            "still in the file",
        );
        if let Some(e) = at(c, w, OUTPOST, "e_brl001") {
            c.check(
                "barrel keeps our name",
                e.name() == "Fuel Barrel",
                format!("named {:?}", e.name()),
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_flood1") {
            num_is(
                c,
                "their floodlight change is kept",
                e.num("Light.intensity"),
                1500.0,
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_gen001") {
            vec_is(
                c,
                "our generator move is kept",
                Some(e.position()),
                &[6.0, 0.0, -5.0],
            );
        }
        let unmerged = w.git(&["ls-files", "-u"]).unwrap_or_default();
        c.check(
            "git has no unmerged paths",
            unmerged.trim().is_empty(),
            unmerged.trim().to_owned(),
        );
        let merging = w
            .git(&["rev-parse", "-q", "--verify", "MERGE_HEAD"])
            .is_some();
        c.check("the merge is committed", !merging, "MERGE_HEAD still set");
        let parents = w
            .git(&["rev-list", "--parents", "-n", "1", "HEAD"])
            .map_or(0, |l| l.split_whitespace().count());
        c.check(
            "HEAD is a merge commit",
            parents == 3,
            format!("{} parents", parents.saturating_sub(1)),
        );
    },
    solve: |s: &mut Solver| {
        let path = authored_path(s.root(), OUTPOST);
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let mut doc: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if let Some(o) = doc.as_object_mut() {
            o.shift_remove("$conflicts");
        }
        s.write_authored(OUTPOST, &ara_json::to_canonical_string(&doc))?;
        s.git(&["add", &format!("{}/{OUTPOST}", ara_doc::AUTHORING_ROOT)])?;
        s.git(&["commit", "-q", "--no-edit"]).map(drop)
    },
};

/// Misspelled field names that the diagnostics offer `rename` fixes for.
pub const FIX_TYPOS: Task = Task {
    id: "diag-fix-typos",
    area: Area::Diagnostics,
    prompt: "`ara check` reports errors in the outpost level. Fix them without losing any of the values that were set.",
    fixture: Fixture::Outpost,
    setup: Some(|root: &Path| {
        patch(root, OUTPOST, "\"size\": [2, 1, 1]", "\"sise\": [2, 1, 1]")?;
        patch(root, OUTPOST, "\"intensity\": 1200,", "\"intensty\": 1200,")
    }),
    budget: Budget::new(25, 6),
    verify: |w: &World, c: &mut Checks| {
        clean(c, w);
        if let Some(e) = at(c, w, OUTPOST, "e_bench1") {
            vec_is(
                c,
                "Workbench collider keeps its 2 x 1 x 1 m size",
                e.vec("Collider.size"),
                &[2.0, 1.0, 1.0],
            );
        }
        if let Some(e) = at(c, w, OUTPOST, "e_wlight1") {
            num_is(
                c,
                "WorkLight keeps 1200 lm",
                e.num("Light.intensity"),
                1200.0,
            );
            num_is(
                c,
                "WorkLight keeps its 6 m range",
                e.num("Light.range"),
                6.0,
            );
        }
    },
    solve: |s: &mut Solver| {
        let path = authored_path(s.root(), OUTPOST);
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        s.write_authored(
            OUTPOST,
            &text
                .replace("\"sise\"", "\"size\"")
                .replace("\"intensty\"", "\"intensity\""),
        )
    },
};
