//! Real Git fixtures cover merges, tag chains, signatures, byte preservation and collisions.
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
static NEXT: AtomicUsize = AtomicUsize::new(0);

fn fixture() -> (PathBuf, git::Git) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/ara-dev/history-tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&dir).unwrap();
    let repo = dir.join("source.git");
    assert!(
        git::command()
            .args(["init", "--bare", "--initial-branch=main"])
            .arg(&repo)
            .output()
            .unwrap()
            .status
            .success()
    );
    (dir, git::Git(repo.canonicalize().unwrap()))
}
fn commit(git: &git::Git, tree: &str, parents: &[&str], message: &[u8], signed: bool) -> String {
    let mut bytes = format!("tree {tree}\n").into_bytes();
    for parent in parents {
        bytes.extend(format!("parent {parent}\n").as_bytes());
    }
    bytes.extend(b"author Original <o@x> 1700000000 +0530\ncommitter Original <o@x> 1700000001 -0400\nencoding ISO-8859-1\n");
    if signed {
        bytes
            .extend(b"gpgsig -----BEGIN PGP SIGNATURE-----\n fake\n -----END PGP SIGNATURE-----\n");
    }
    bytes.push(b'\n');
    bytes.extend(message);
    git.write("commit", &bytes).unwrap()
}

#[test]
fn isolated_rehearsal_verifies_every_merge_and_tag_and_matches_the_old_fork() {
    let (dir, source) = fixture();
    let tree = String::from_utf8(source.input(&["mktree"], &[]).unwrap())
        .unwrap()
        .trim()
        .to_owned();
    let root = commit(
        &source,
        &tree,
        &[],
        b"Root \xff\r\n\r\nCo-Authored-By: Claude <bot@anthropic.com>\r\n\r\n",
        true,
    );
    let left = commit(
        &source,
        &tree,
        &[&root],
        b"Left\n\nBody without final newline",
        false,
    );
    let right = commit(
        &source,
        &tree,
        &[&root],
        b"Right\n\nGenerated with Claude Code\n",
        false,
    );
    let merge = commit(
        &source,
        &tree,
        &[&left, &right],
        b"Merge\n\nCo-authored-by: Alice <a@b>\n\n",
        false,
    );
    let tag = source.write("tag", format!("object {merge}\ntype commit\ntag signed\ntagger T <t@x> 1700000002 +0300\n\nRelease\n\nCo-Authored-By: Claude\n\n-----BEGIN PGP SIGNATURE-----\nfake\n-----END PGP SIGNATURE-----\n").as_bytes()).unwrap();
    let nested = source
        .write(
            "tag",
            format!(
                "object {tag}\ntype tag\ntag nested\ntagger T <t@x> 1700000003 +0300\n\nNested\n"
            )
            .as_bytes(),
        )
        .unwrap();
    for (name, id) in [
        ("refs/heads/main", &merge),
        ("refs/heads/feature", &left),
        ("refs/tags/light", &root),
        ("refs/tags/signed", &tag),
        ("refs/tags/nested", &nested),
        ("refs/remotes/origin/main", &merge),
    ] {
        source.run(&["update-ref", name, id]).unwrap();
    }
    source
        .run(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ])
        .unwrap();
    let untouched = source.text(&["show-ref"]).unwrap();
    let scratch = dir.join("rehearsal");
    assert!(
        run(Options {
            source: source.0.to_string_lossy().into(),
            scratch: scratch.clone()
        })
        .unwrap()
    );
    assert_eq!(source.text(&["show-ref"]).unwrap(), untouched);
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(scratch.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["commits_before"], 4);
    assert_eq!(report["commits_after"], 4);
    assert_eq!(report["trees_verified"], 4);
    assert_eq!(report["ordered_parent_lists_verified"], 4);
    assert_eq!(report["commit_messages_changed"], 2);
    assert_eq!(report["tags"], 3);
    assert_eq!(report["annotated_tags"], 2);
    assert_eq!(report["tag_messages_changed"], 1);
    assert_eq!(report["signatures"].as_array().unwrap().len(), 2);
    assert!(
        report["signatures"]
            .as_array()
            .unwrap()
            .iter()
            .all(|s| s["invalidated"] == true)
    );
    assert!(
        match_base(MatchOptions {
            old_repo: source.0.clone(),
            old_base: root,
            new_repo: scratch.join("mirror.git"),
            new_tip: "main".into()
        })
        .unwrap()
    );
    assert!(
        run(Options {
            source: source.0.to_string_lossy().into(),
            scratch
        })
        .is_err()
    );
    assert!(source.validate_isolation().is_err());
}

#[test]
fn an_identity_collision_fails_before_refs_change() {
    let (dir, source) = fixture();
    let tree = String::from_utf8(source.input(&["mktree"], &[]).unwrap())
        .unwrap()
        .trim()
        .to_owned();
    let first = commit(
        &source,
        &tree,
        &[],
        b"Same\n\nCo-Authored-By: Claude\n",
        false,
    );
    let second = commit(
        &source,
        &tree,
        &[],
        b"Same\n\nCo-Authored-By: Anthropic\n",
        false,
    );
    source
        .run(&["update-ref", "refs/heads/main", &first])
        .unwrap();
    source
        .run(&["update-ref", "refs/heads/other", &second])
        .unwrap();
    let scratch = dir.join("collision");
    let problem = run(Options {
        source: source.0.to_string_lossy().into(),
        scratch: scratch.clone(),
    })
    .unwrap_err();
    assert!(problem.to_string().contains("collapse"));
    assert_eq!(
        git::Git(scratch.join("mirror.git"))
            .text(&["show-ref"])
            .unwrap(),
        source.text(&["show-ref"]).unwrap()
    );
}

#[test]
fn fork_matching_rejects_missing_and_repeated_full_messages() {
    let (_dir, source) = fixture();
    let tree = String::from_utf8(source.input(&["mktree"], &[]).unwrap())
        .unwrap()
        .trim()
        .to_owned();
    let root = commit(&source, &tree, &[], b"Root\n", false);
    let first = commit(&source, &tree, &[&root], b"Repeated\n", false);
    let second = commit(&source, &tree, &[&first], b"Repeated\n", false);
    source
        .run(&["update-ref", "refs/heads/main", &second])
        .unwrap();
    for (tip, expected) in [(&root, "found 0"), (&second, "found 2")] {
        let problem = match_base(MatchOptions {
            old_repo: source.0.clone(),
            old_base: first.clone(),
            new_repo: source.0.clone(),
            new_tip: tip.clone(),
        })
        .unwrap_err();
        assert!(problem.to_string().contains(expected));
    }
}
