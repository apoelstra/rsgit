//
// Copyright (c) 2021
//      Andrew Poelstra <rsgit@wpsoftware.net>
//
// This program is free software; you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation; either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program; if not, write to the Free Software
// Foundation, Inc., 675 Mass Ave, Cambridge, MA 02139, USA.
//

mod pr;

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::usize;

use anyhow::Context;
use git2::{Repository, Signature};
use structopt::StructOpt;

use self::pr::PullRequest;

#[derive(StructOpt, Debug)]
struct Opts {
    /// The repository to tag PRs in
    #[structopt(short = "r", long = "repo", default_value = ".")]
    repo: String,
    /// Label structure to apply in the form pr_ref:master,branches:url_prefix
    #[structopt(name = "labels")]
    labels: Vec<Label>,
}

#[derive(Debug)]
struct Label {
    /// The URL to use as a prefix when linking to PRs
    url_prefix: String,
    /// The prefix to search for PR refs under
    pr_ref: String,
    /// List of master branches (defaults to just 'master')
    master: Vec<String>,
}

impl FromStr for Label {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let mut segments = s.splitn(3, ':');
        let pr_ref = match segments.next() {
            Some(pr_ref) => pr_ref.into(),
            None => return Err(format!("missing pr_ref field in {}", s)),
        };
        let master = match segments.next() {
            Some(branches) => branches.split(',').map(String::from).collect(),
            None => return Err(format!("missing branches field in {}", s)),
        };
        let url_prefix = match segments.next() {
            Some(prefix) => prefix.into(),
            None => return Err(format!("missing url_prefix field in {}", s)),
        };
        Ok(Label {
            url_prefix,
            pr_ref,
            master,
        })
    }
}

struct Note<'label> {
    url_prefix: &'label str,
    pr_num: usize,
    commit_index: usize,
    n_commits: usize,
}

fn main() -> anyhow::Result<()> {
    let opts = Opts::from_args();
    let repo = Repository::open_ext(&opts.repo, git2::RepositoryOpenFlags::empty(), Some("/"))
        .with_context(|| format!("Opening repo {}", opts.repo))?;

    for label in &opts.labels {
        // 1. Collect PRs
        let mut prs = vec![];
        println!(
            "Labeling {} from refs/remotes/{} (master branch {:?})",
            label.url_prefix, label.pr_ref, label.master
        );
        'ref_loop: for rf in repo.references().expect("get references") {
            let rf = rf.expect("reference is legit");
            if rf.is_remote() {
                let name = rf.name().unwrap();
                let mut segments = name.split('/');
                if segments.next() != Some("refs") {
                    continue;
                }
                if segments.next() != Some("remotes") {
                    continue;
                }
                for seg in label.pr_ref.split('/') {
                    if segments.next() != Some(seg) {
                        continue 'ref_loop;
                    }
                }
                let num = match segments.next().map(usize::from_str) {
                    Some(Ok(n)) => n,
                    _ => continue,
                };
                if segments.next() != Some("head") {
                    continue;
                }
                prs.push(PullRequest {
                    number: num,
                    id: rf.target().expect("dereference pr ref"),
                });
            }
        }
        println!("Found {} PRs", prs.len());

        // 2. Check master tree
        let mut parent_commits = HashSet::new();
        for master in &label.master {
            let rf = repo.revparse_single(master).expect("look up master ref");
            let mut parent = repo.find_commit(rf.id());
            while let Ok(parent_commit) = parent {
                parent_commits.insert(parent_commit.id());
                parent = parent_commit.parent(0);
            }
        }
        println!("Found {} parent commits", parent_commits.len());

        // 3. Build map of notes
        let mut note_map = HashMap::new();
        for (n, pr) in prs.iter().enumerate() {
            pr.for_each_commit(&repo, &parent_commits, |id, index, n_commits| {
                note_map.entry(id).or_insert(vec![]).push(Note {
                    url_prefix: &label.url_prefix,
                    pr_num: pr.number,
                    commit_index: n_commits - index,
                    n_commits: n_commits,
                })
            });

            if n % 10_000 == 9_999 || n == prs.len() - 1 {
                println!(
                    "Labelling {} commits ({} / {} PRs)",
                    note_map.len(),
                    n + 1,
                    prs.len()
                );
                create_notes(&repo, note_map)?;
                note_map = HashMap::new();
            }
        }
    }

    Ok(())
}

fn create_notes(
    repo: &Repository,
    mut note_map: HashMap<git2::Oid, Vec<Note>>,
) -> anyhow::Result<()> {
    // 4. Build note commit
    let mut note_tree = repo.treebuilder(None).expect("getting a treebuilder");
    for (id, notes) in &mut note_map {
        let mut msg = String::new();
        notes.sort_by_key(|note| (note.url_prefix, note.pr_num));

        for note in notes {
            msg.push_str(&format!(
                "PR: {}{} ({}/{})\n",
                note.url_prefix, note.pr_num, note.commit_index, note.n_commits
            ));
        }
        let blob_id = repo.blob(msg.as_bytes()).expect("writing note blob");
        note_tree
            .insert(id.to_string(), blob_id, 33188)
            .expect("putting note blob in tree");
    }
    let note_tree_id = note_tree.write().expect("writing new note tree");
    let note_tree = repo
        .find_tree(note_tree_id)
        .expect("reading tree we just wrote");

    // 5. Put notes into repo
    let mut parents = vec![];
    if let Ok(existing) = repo.find_reference("refs/notes/label-pr") {
        parents.push(
            existing
                .peel_to_commit()
                .expect("existing ref points to commit"),
        );
    }
    let parents_refs: Vec<&_> = parents.iter().collect(); // we need a slice of references for `commit()`
    let sig = Signature::now("PR Labeller", "prlabel@wpsoftware.net").expect("create sig");
    let comm_id = repo
        .commit(
            Some("refs/notes/label-pr"),
            &sig,
            &sig,
            "Notes added by label-pr utility",
            &note_tree,
            &parents_refs,
        )
        .expect("committing new notes");

    println!("Done. Added new notes as {}", comm_id);
    Ok(())
}
