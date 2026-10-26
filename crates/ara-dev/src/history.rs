//! An isolated mirror rehearsal; never rewrite a working checkout or push anything.
mod git;
mod message;
mod object;
#[cfg(test)]
mod tests;

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::time::Instant;

#[derive(clap::Args)]
pub(crate) struct Options {
    /// Repository path or remote URL; cloned read-only with all advertised refs.
    #[arg(long)]
    source: String,
    /// New, nonexistent directory; contains mirror.git and verification evidence.
    #[arg(long)]
    scratch: PathBuf,
}

#[derive(clap::Args)]
pub(crate) struct MatchOptions {
    #[arg(long)]
    old_repo: PathBuf,
    #[arg(long)]
    old_base: String,
    #[arg(long)]
    new_repo: PathBuf,
    #[arg(long, default_value = "origin/main")]
    new_tip: String,
}

/// Read-only recovery: a subject alone is never enough to identify a fork.
pub(crate) fn match_base(options: MatchOptions) -> Result<bool> {
    let old = git::Git(options.old_repo.canonicalize()?);
    let new = git::Git(options.new_repo.canonicalize()?);
    let old_id = old.text(&[
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{}^{{commit}}", options.old_base),
    ])?;
    let tip = new.text(&[
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{}^{{commit}}", options.new_tip),
    ])?;
    let original = old.batch()?.read(&old_id)?;
    let identity = object::identity(&original.bytes)?;
    let mut reader = new.batch()?;
    let mut found = Vec::new();
    for id in new.text(&["rev-list", &tip])?.lines() {
        if object::identity(&reader.read(id)?.bytes)? == identity {
            found.push(id.to_owned());
        }
    }
    ensure!(
        found.len() == 1,
        "expected one full-message/tree/metadata match for {old_id}; found {} (never guess by subject)",
        found.len()
    );
    println!(
        "{}",
        serde_json::json!({"old":old_id,"new":found[0],"tip":tip})
    );
    Ok(true)
}

struct Reference {
    name: String,
    oid: String,
    symbolic: String,
}
fn references(git: &git::Git) -> Result<Vec<Reference>> {
    git.text(&[
        "for-each-ref",
        "--format=%(refname)%09%(objectname)%09%(symref)",
    ])?
    .lines()
    .map(|line| {
        let fields: Vec<_> = line.split('\t').collect();
        ensure!(fields.len() == 3, "invalid ref inventory");
        Ok(Reference {
            name: fields[0].into(),
            oid: fields[1].into(),
            symbolic: fields[2].into(),
        })
    })
    .collect()
}
fn commits(git: &git::Git) -> Result<Vec<String>> {
    Ok(git
        .text(&["rev-list", "--all", "--topo-order", "--reverse"])?
        .lines()
        .map(str::to_owned)
        .collect())
}

#[derive(Serialize)]
struct Signature {
    kind: String,
    old: String,
    new: String,
    invalidated: bool,
}
#[derive(Serialize)]
struct Report {
    format: &'static str,
    source: String,
    mirror: PathBuf,
    source_head: String,
    commits_before: usize,
    commits_after: usize,
    commits_rehashed: usize,
    commit_messages_changed: usize,
    annotated_tags: usize,
    tag_messages_changed: usize,
    refs: usize,
    branches: usize,
    remote_branches: usize,
    tags: usize,
    trees_verified: usize,
    ordered_parent_lists_verified: usize,
    metadata_and_messages_verified: usize,
    remaining_matches: usize,
    signatures: Vec<Signature>,
    clone_seconds: f64,
    rewrite_seconds: f64,
    verification_seconds: f64,
    total_seconds: f64,
}

struct Rewrite<'a> {
    git: &'a git::Git,
    reader: git::Batch,
    map: BTreeMap<String, String>,
    originals: BTreeMap<String, git::Object>,
    signatures: Vec<Signature>,
    changed_commits: usize,
    changed_tags: usize,
}
impl<'a> Rewrite<'a> {
    fn new(git: &'a git::Git) -> Result<Self> {
        Ok(Self {
            git,
            reader: git.batch()?,
            map: BTreeMap::new(),
            originals: BTreeMap::new(),
            signatures: Vec::new(),
            changed_commits: 0,
            changed_tags: 0,
        })
    }
    fn write(&mut self, old: &str, object: git::Object, key: &[u8]) -> Result<String> {
        let rewritten = object::rewrite(&object.bytes, key, &self.map)?;
        let changed_message = object::parts(&object.bytes)?.1 != object::parts(&rewritten)?.1;
        if object.kind == "commit" {
            self.changed_commits += usize::from(changed_message);
        } else {
            self.changed_tags += usize::from(changed_message);
        }
        let new = if rewritten == object.bytes {
            old.to_owned()
        } else {
            self.git.write(&object.kind, &rewritten)?
        };
        if object::signed(&object.kind, &object.bytes)? {
            self.signatures.push(Signature {
                kind: object.kind.clone(),
                old: old.into(),
                new: new.clone(),
                invalidated: new != old,
            });
        }
        self.map.insert(old.into(), new.clone());
        self.originals.insert(old.into(), object);
        Ok(new)
    }
    fn target(&mut self, old: &str, depth: usize) -> Result<String> {
        if let Some(new) = self.map.get(old) {
            return Ok(new.clone());
        }
        ensure!(depth < 128, "annotated tag nesting exceeds 128");
        let object = self.reader.read(old)?;
        if object.kind == "tag" {
            let targets = object::links(&object.bytes, b"object ")?;
            ensure!(targets.len() == 1, "tag must have one target");
            self.target(&targets[0], depth + 1)?;
            self.write(old, object, b"object ")
        } else {
            ensure!(
                object.kind != "commit",
                "commit was not visited in parent order: {old}"
            );
            self.map.insert(old.into(), old.into());
            Ok(old.into())
        }
    }
}

pub(crate) fn run(options: Options) -> Result<bool> {
    let began = Instant::now();
    ensure!(
        !options.scratch.exists(),
        "scratch directory must not already exist"
    );
    fs::create_dir(&options.scratch)
        .context("create isolated rehearsal directory (parent must exist)")?;
    let scratch = options.scratch.canonicalize()?;
    eprintln!(
        "Cloning into {} (independent objects, push disabled)",
        scratch.display()
    );
    let git = git::clone(&options.source, &scratch.join("mirror.git"))?;
    let clone_seconds = began.elapsed().as_secs_f64();
    let refs = references(&git)?;
    let before = commits(&git)?;
    let head = git.text(&["rev-parse", "HEAD"])?;
    let head_ref = git.text(&["symbolic-ref", "HEAD"])?;
    ensure!(
        refs.iter().any(|r| r.name == head_ref),
        "HEAD target absent from cloned refs"
    );
    let rewriting = Instant::now();
    let mut rewrite = Rewrite::new(&git)?;
    for (index, id) in before.iter().enumerate() {
        let object = rewrite.reader.read(id)?;
        ensure!(object.kind == "commit", "rev-list returned a non-commit");
        rewrite.write(id, object, b"parent ")?;
        if (index + 1) % 250 == 0 {
            eprintln!("Rewritten {}/{} commits", index + 1, before.len());
        }
    }
    for reference in &refs {
        rewrite.target(&reference.oid, 0)?;
    }
    let rewrite_seconds = rewriting.elapsed().as_secs_f64();
    let verifying = Instant::now();
    let mapped: BTreeSet<_> = before.iter().map(|old| rewrite.map[old].clone()).collect();
    ensure!(
        mapped.len() == before.len(),
        "rewrite would collapse distinct commits; refs remain unchanged"
    );
    let mut commit_map = String::from("old\tnew\told_tree\tnew_tree\told_parents\tnew_parents\n");
    let mut tag_count = 0;
    for (old, object) in &rewrite.originals {
        let new = &rewrite.map[old];
        let after = rewrite.reader.read(new)?;
        ensure!(after.kind == object.kind, "object kind changed");
        let key = if object.kind == "commit" {
            b"parent ".as_slice()
        } else {
            tag_count += 1;
            b"object ".as_slice()
        };
        object::verify(&object.bytes, &after.bytes, key, &rewrite.map)?;
        if object.kind == "commit" {
            let tree = object::links(&object.bytes, b"tree ")?;
            ensure!(
                tree.len() == 1 && tree == object::links(&after.bytes, b"tree ")?,
                "tree changed for {old}"
            );
            commit_map.push_str(&format!(
                "{old}\t{new}\t{}\t{}\t{}\t{}\n",
                tree[0],
                tree[0],
                object::links(&object.bytes, b"parent ")?.join(","),
                object::links(&after.bytes, b"parent ")?.join(",")
            ));
        }
    }
    fs::write(scratch.join("commit-map.tsv"), commit_map)?;
    let mut ref_map = String::from("ref\told\tnew\tsymbolic\n");
    let mut transaction = String::from("start\n");
    for reference in &refs {
        let new = &rewrite.map[&reference.oid];
        ref_map.push_str(&format!(
            "{}\t{}\t{new}\t{}\n",
            reference.name, reference.oid, reference.symbolic
        ));
        if reference.symbolic.is_empty() {
            transaction.push_str(&format!(
                "update {} {new} {}\n",
                reference.name, reference.oid
            ));
        }
    }
    fs::write(scratch.join("ref-map.tsv"), ref_map)?;
    transaction.push_str("prepare\ncommit\n");
    git.input(&["update-ref", "--stdin"], transaction.as_bytes())?;
    let after: BTreeSet<_> = commits(&git)?.into_iter().collect();
    ensure!(after == mapped, "reachable commit count/set changed");
    let after_refs = references(&git)?;
    ensure!(after_refs.len() == refs.len(), "ref count changed");
    for (old, new) in refs.iter().zip(&after_refs) {
        ensure!(
            old.name == new.name
                && old.symbolic == new.symbolic
                && rewrite.map[&old.oid] == new.oid,
            "ref/symbolic topology changed"
        );
    }
    ensure!(
        git.text(&["symbolic-ref", "HEAD"])? == head_ref
            && git.text(&["rev-parse", "HEAD"])? == rewrite.map[&head],
        "HEAD topology changed"
    );
    let fsck = git.fsck()?;
    fs::write(scratch.join("fsck.log"), fsck)?;
    let report = Report {
        format: "ara.history-rehearsal/1",
        source: options.source,
        mirror: git.0.clone(),
        source_head: head,
        commits_before: before.len(),
        commits_after: after.len(),
        commits_rehashed: before
            .iter()
            .filter(|old| rewrite.map[*old] != **old)
            .count(),
        commit_messages_changed: rewrite.changed_commits,
        annotated_tags: tag_count,
        tag_messages_changed: rewrite.changed_tags,
        refs: refs.len(),
        branches: refs
            .iter()
            .filter(|r| r.name.starts_with("refs/heads/"))
            .count(),
        remote_branches: refs
            .iter()
            .filter(|r| r.name.starts_with("refs/remotes/") && r.symbolic.is_empty())
            .count(),
        tags: refs
            .iter()
            .filter(|r| r.name.starts_with("refs/tags/"))
            .count(),
        trees_verified: before.len(),
        ordered_parent_lists_verified: before.len(),
        metadata_and_messages_verified: before.len() + tag_count,
        remaining_matches: 0,
        signatures: rewrite.signatures,
        clone_seconds,
        rewrite_seconds,
        verification_seconds: verifying.elapsed().as_secs_f64(),
        total_seconds: began.elapsed().as_secs_f64(),
    };
    fs::write(
        scratch.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(true)
}
