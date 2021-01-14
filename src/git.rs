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

use anyhow::{self, Context};
use git2::{self, Repository, Tree};
use std::borrow::Cow;
use std::fs;

/// A structure representing a temporary worktree of the repository.
/// When it is dropped the worktree will be removed
pub struct TempWorktree {
    /// The git worktree object
    pub worktree: git2::Worktree,
    /// The directory it's contained in
    pub dir: tempfile::TempDir,
}

impl TempWorktree {
    /// Creates a new temporary worktree in a given repository
    pub fn new(repo: &Repository, head: Option<&git2::Reference>) -> anyhow::Result<Self> {
        let new_dir = tempfile::tempdir()
            .context("creating temporary directory for new worktree")?;
	let name = format!(
            "checkpr-temp-worktree-{}",
            new_dir.path().file_name().and_then(|oss| oss.to_str()).unwrap_or(""),
        );
        fs::remove_dir(new_dir.path())
            .context("removing temp dir so that git-worktree can recreate it")?;
        let worktree = repo.worktree(
            &name,
            new_dir.path(),
            Some(git2::WorktreeAddOptions::new().reference(head)),
        ).with_context(|| format!("creating new worktree {}", name))?;

        Ok(TempWorktree {
            worktree: worktree,
            dir: new_dir,
        })
    }

    /// Attempt to open the worktree as a repository
    pub fn repo(&self) -> anyhow::Result<Repository> {
        Repository::open_from_worktree(&self.worktree)
            .with_context(|| format!("opening worktree at {} as repo", self.dir.path().to_string_lossy()))
            .map_err(anyhow::Error::from)
    }

    /// Accessor for the path as a unicode string
    ///
    /// If the underlying path has non-unicode characters they are
    /// replaced by `U+FFFD REPLACEMENT CHARACTER`
    pub fn path(&self) -> Cow<str> {
        self.dir.path().to_string_lossy()
    }
}

impl Drop for TempWorktree {
    fn drop(&mut self) {
        // prune valid worktree .. it won't be valid soon when we delete it!
        if let Err(e) = self.worktree.prune(Some(
            &mut git2::WorktreePruneOptions::new().locked(true).valid(true)
        )) {
            eprintln!(
                "WARNING: failed to remove worktree at {}: {}",
                self.dir.path().to_string_lossy(),
                e,
            );
        }
    }
}

/// A structure representing a temporary repository. When
/// it is dropped the repository will be deleted from disk.
pub struct TempRepo {
    /// The git repository
    pub repo: git2::Repository,
    /// The directory it's contained in
    pub dir: tempfile::TempDir,
}

impl TempRepo {
    /// Creates a new temporary repo
    pub fn new() -> anyhow::Result<Self> {
        let new_repo_dir = tempfile::tempdir()
            .context("creating temporary directory for new repo")?;
        let path_str = new_repo_dir.path().to_string_lossy();
        let new_repo = Repository::init(new_repo_dir.path())
            .with_context(|| format!("initializing temporary repo in {}", path_str))?;

        Ok(TempRepo {
            repo: new_repo,
            dir: new_repo_dir,
        })
    }

    /// Copy an entire tree from a source repo and check it out
    pub fn copy_tree_and_checkout<'src>(
        &self,
        tree: &Tree<'src>,
        source: &'src Repository,
    ) -> anyhow::Result<()> {
        // Do the copy
        copy_tree(source, &self.repo, tree)?;

        // Convert to an index to do the checkout
        let mut index = git2::Index::new()
            .context("Creating in-memory index")?;
        index.read_tree(&tree)
            .with_context(|| format!("reading tree {} into index", tree.id()))?;

        let new_id = index.write_tree_to(&self.repo)
            .with_context(|| format!("writing tree {} into index", tree.id()))?;
        let new_obj = self.repo.find_object(new_id, None)
            .with_context(|| format!("finding object {} that we just created", new_id))?;
        self.repo.checkout_tree(&new_obj, None)
            .with_context(|| format!("checking out {}", new_id))?;

        Ok(())
    }

    /// Accessor for the path as a unicode string
    ///
    /// If the underlying path has non-unicode characters they are
    /// replaced by `U+FFFD REPLACEMENT CHARACTER`
    pub fn path(&self) -> Cow<str> {
        self.dir.path().to_string_lossy()
    }
}

/// Creates a new temporary repo and copies the specified commit ID into it
pub fn temp_repo<'src>(
    source: &'src Repository,
    commit_id: git2::Oid,
) -> anyhow::Result<TempRepo> {
    // Create the reop
    let commit = source.find_commit(commit_id)
        .with_context(|| format!("finding commit {}", commit_id))?;
    let tree = commit.tree()
        .with_context(|| format!("getting tree for {}", commit_id))?;

    let new_repo = TempRepo::new()?;
    new_repo.copy_tree_and_checkout(&tree, source)
        .with_context(|| format!("copying commit {}'s tree to {}", commit_id, new_repo.path()))?;


    println!("Created new repo in {} with commit {} read into it", new_repo.path(), commit_id);
    Ok(new_repo)
}

/// Copy a tree from one repo into another
fn copy_tree<'src, 'dst>(
    source: &'src Repository,
    dest: &'dst Repository,
    tree: &Tree<'src>,
) -> anyhow::Result<()> {
    let mut abort_err = Ok(());
    let src_odb = source.odb().context("getting odb for source repo")?;
    let dst_odb = dest.odb().context("getting odb for dest repo")?;

    tree.walk(
        git2::TreeWalkMode::PreOrder,
        |_, entry| {
            let obj = match src_odb.read(entry.id()) {
                Ok(obj) => obj,
                Err(e) => {
                    abort_err = Err(e)
                        .with_context(|| format!("getting object {}", entry.id()));
                    return git2::TreeWalkResult::Abort;
                },
            };
            let new_id = match dst_odb.write(obj.kind(), obj.data()) {
                Ok(id) => id,
                Err(e) => {
                    abort_err = Err(e)
                        .with_context(|| format!("writing object {}", entry.id()));
                    return git2::TreeWalkResult::Abort;
                },
            };
            assert_eq!(new_id, entry.id());
            git2::TreeWalkResult::Ok
        }
    ).with_context(|| format!("walking tree {}", tree.id()))?;
    abort_err?;

    // Copy the tree itself 
    let obj = src_odb.read(tree.id())
        .with_context(|| format!("reading tree {} as ODB object", tree.id()))?;
    let new_id = dst_odb.write(obj.kind(), obj.data())
        .with_context(|| format!("writing tree {} as ODB object", tree.id()))?;
    assert_eq!(new_id, tree.id());


    Ok(())
}
    

