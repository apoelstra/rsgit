
use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;
use std::str::FromStr;
use std::usize;

use structopt::StructOpt;
use git2::{Oid, Repository, Signature};

#[derive(StructOpt, Debug)]
struct Opts {
    /// The repository to tag PRs in
    #[structopt(short="r", long="repo", default_value=".")]
    repo: String,
    /// Label structure to apply in the form pr_ref:master,branches:url_prefix
    #[structopt(name="labels")]
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
        Ok(Label { url_prefix, pr_ref, master })
    }
}

struct PullRequest {
    number: usize,
    id: Oid,
}

struct Note<'label> {
    url_prefix: &'label str,
    pr_num: usize,
    commit_index: usize,
    n_commits: usize,
}

impl PullRequest {
    fn get_ancestors<'label>(
        &self,
        repo: &Repository,
        master_commits: &HashSet<Oid>,
        note_map: &mut HashMap<Oid, Vec<Note<'label>>>,
        url_prefix: &'label str,
    ) {
        let mut pr_map = HashMap::new();

        let mut stack = vec![vec![repo.find_commit(self.id).expect("look up commit")]];
        let mut idx = 0;
        while let Some(tips) = stack.pop() {
            for tip in tips {
                match pr_map.entry(tip.id()) {
                    Entry::Occupied(_) => continue, // already seen
                    Entry::Vacant(vac) => vac.insert(idx),
                };
                idx += 1;

                let mut parent_vec = Vec::with_capacity(tip.parent_count());
                for parent in tip.parents() {
                    if !master_commits.contains(&parent.id()) {
                        parent_vec.push(parent);
                    }
                }
                if !parent_vec.is_empty() {
                    stack.push(parent_vec);
                }
            }
        }

        let n_commits = pr_map.len();
        for (id, index) in pr_map {
            note_map.entry(id).or_insert(vec![]).push(Note {
                url_prefix: url_prefix,
                pr_num: self.number,
                commit_index: n_commits - index,
                n_commits: n_commits,
            });
        }
    }
}

fn main() {
    let opts = Opts::from_args();
    let repo = Repository::open(&opts.repo).expect("open repo");

    let mut note_map = HashMap::new();
    for label in &opts.labels {
        // 1. Collect PRs
        let mut prs = vec![];
        println!("Labeling {} from refs/remotes/{} (master branch {:?})", label.url_prefix, label.pr_ref, label.master);
        'ref_loop: for rf in repo.references().expect("get references") {
            let rf = rf.expect("reference is legit");
            if rf.is_remote() {
                let name = rf.name().unwrap();
                let mut segments = name.split('/');
                if segments.next() != Some("refs") { continue; }
                if segments.next() != Some("remotes") { continue; }
                for seg in label.pr_ref.split('/') {
                     if segments.next() != Some(seg) { continue 'ref_loop; }
                }
                let num = match segments.next().map(usize::from_str) {
                    Some(Ok(n)) => n,
                    _ => continue,
                };
                if segments.next() != Some("head") { continue; }
                prs.push(PullRequest {
                    number: num,
                    id: rf.target().expect("dereference pr ref"),
                })
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
        for pr in &prs {
            pr.get_ancestors(&repo, &parent_commits, &mut note_map, &label.url_prefix);
        }
        println!("Labelling {} commits", note_map.len());
    }

    // 4. Attach notes
    for (count, (id, mut notes)) in note_map.into_iter().enumerate() {
        if count > 0 && count % 5000 == 0 {
            println!("{}..", count);
        }

        let mut msg = String::new();
        notes.sort_by_key(|note| (note.url_prefix, note.pr_num));
        for note in &notes {
            msg.push_str(&format!("PR: {}{} ({}/{})\n", note.url_prefix, note.pr_num, note.commit_index, note.n_commits));
        }
        if let Ok(existing) = repo.find_note(Some("refs/notes/label-pr"), id) {
            if existing.message() == Some(&msg) {
                continue; // done already
            }
        }
        let sig = Signature::now("PR Labeller", "prlabel@wpsoftware.net").expect("create sig");
        repo.note(&sig, &sig, Some("refs/notes/label-pr"), id, &msg, true).expect("adding note");
    }
    println!("Done");
}
