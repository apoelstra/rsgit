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

mod checks;
mod git;
mod pr;

use std::collections::HashSet;

use anyhow::Context;
use git2::Repository;
use structopt::StructOpt;

use self::pr::PullRequest;

#[derive(StructOpt, Debug)]
struct Opts {
    /// Repository to read
    #[structopt(short, long, default_value = ".")]
    repo: String,
    /// The tip of the PR to check
    #[structopt(short, long)]
    tip: String,
    /// The "master" branch the PR was forked from
    #[structopt(short, long, default_value = "master")]
    master: String,
    /// Whether to accept PRs that have merge commits in them. We cannot
    /// do rebase-testing of these.
    #[structopt(long)]
    allow_merges: bool,
}

fn main() -> anyhow::Result<()> {
    let opts = Opts::from_args();
    let repo = Repository::open_ext(
        &opts.repo,
        git2::RepositoryOpenFlags::empty(),
        Option::<String>::None,
    )
    .with_context(|| format!("Opening repo {}", opts.repo))?;

    // 1. Compute first-parent history of master to determine where
    //    the fork point of the PR was
    let mut parent_commits = HashSet::new();
    let rf = repo
        .revparse_single(&opts.master)
        .with_context(|| format!("looking up master ref {}", opts.master))?;

    let master_id = rf.id();
    let master_tip = repo
        .find_commit(master_id)
        .with_context(|| format!("reading master oid {} as a commit", master_id))?;
    let mut parent = Ok(master_tip.clone());
    while let Ok(parent_commit) = parent {
        parent_commits.insert(parent_commit.id());
        parent = parent_commit.parent(0);
    }
    println!(
        "Found {} parent commits starting from master {}",
        parent_commits.len(),
        master_id
    );

    // 2. Get set of commits in the PR (you can use label-pr to assign
    //    some sort of ordering to them, but for our purposes here we
    //    just test them all and don't care about the order).
    let rf = repo
        .revparse_single(&opts.tip)
        .with_context(|| format!("looking up PR tip ref {}", opts.tip))?;
    let pr_id = rf.id();
    let pr_tip = repo
        .find_commit(pr_id)
        .with_context(|| format!("reading PR tip oid {} as commit", rf.id()))?;

    let mut pr_linear_commits = vec![];
    let mut has_merges = false;
    let mut needs_rebase = true;
    let mut parent = Ok(pr_tip.clone());
    while let Ok(parent_commit) = parent {
        let id = parent_commit.id();
        if parent_commits.contains(&id) {
            if id == master_id {
                needs_rebase = false;
            }
            break;
        }

        if parent_commit.parent_count() > 1 {
            has_merges = true;
            println!("Note: commit {} is a merge commit.", id);
        }
        parent = parent_commit.parent(0);
        pr_linear_commits.push(parent_commit);
    }

    // Alert user about merge/rebaseability story
    if needs_rebase {
        println!("Note: PR is not based on master.");
    }
    if needs_rebase && has_merges {
        println!("Note: PR is not based on master, but we cannot do rebase-testing as it contains merges.");
    }
    if !opts.allow_merges && has_merges {
        return Err(anyhow::Error::msg(
            "Refusing to check a PR with merges. Use --allow-merges to allow.",
        ));
    }

    // 3. Construct rebase commits, if needed and possible
    let mut pr_commit_set = HashSet::with_capacity(2 * pr_linear_commits.len());
    if needs_rebase && !has_merges {
        let worktree = self::git::TempWorktree::new(&repo, None)
            .context("creating temporary worktree to do rebase in")?;
        let wt_repo = worktree
            .repo()
            .context("getting temporary worktree as repo")?;

        wt_repo
            .set_head_detached(master_tip.id())
            .with_context(|| format!("setting rebase worktree to master {}", master_tip.id()))?;
        wt_repo
            .checkout_head(None)
            .context("checking out HEAD in rebase worktree")?;

        for commit in &pr_linear_commits {
            let current_head = wt_repo.head().context("getting HEAD")?.target().unwrap();

            let mut merge_opts = git2::MergeOptions::new();
            merge_opts.fail_on_conflict(true);
            wt_repo
                .cherrypick(
                    commit,
                    Some(git2::CherrypickOptions::new().merge_opts(merge_opts)),
                )
                .with_context(|| format!("cherry-picking {} onto {}", commit.id(), current_head))?;

            let new_head = wt_repo.head().context("getting HEAD")?.target().unwrap();
            if new_head == old_head {
                println!("Skipping cherry-pick of {} (no change).", commit.id());
            } else {
                pr_commit_set.insert(new_head);
                println!(
                    "Cherry-picked {} onto {} as {}.",
                    commit.id(),
                    current_head,
                    new_head
                );
            }
        }
    }

    // 4. Put original commits into our set
    PullRequest {
        number: 0, // irrelevant for us
        id: pr_id,
    }
    .for_each_commit(&repo, &parent_commits, |id, _, _| {
        pr_commit_set.insert(id);
    });

    // 5. Spawn new repos for all of our checks
    for id in pr_commit_set {
        let fresh_repo = self::git::temp_repo(&repo, id)
            .with_context(|| format!("creating temporary repo for {}", id))?;
    }

    Ok(())
}
