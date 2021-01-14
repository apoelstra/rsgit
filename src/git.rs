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

/// Creates a new temporary repo and copies the specified commit ID into it
pub fn temp_repo<'src>(
    source: &'src Repository,
    commit_id: git2::Oid,
) -> anyhow::Result<Repository> {
    let commit = source.find_commit(commit_id)
        .with_context(|| format!("finding commit {}", commit_id))?;
    let tree = commit.tree()
        .with_context(|| format!("getting tree for {}", commit_id))?;

    let new_repo_dir = tempfile::tempdir()
        .with_context(|| format!("Creating temporary directory for {}", commit_id))?;
    let path_str = new_repo_dir.path().to_string_lossy();
    let new_repo = Repository::init(new_repo_dir.path())
        .with_context(|| format!("Initializing repo for {} in {}", commit_id, path_str))?;

    copy_tree(source, &new_repo, &tree)
        .with_context(|| format!("copying commit {}'s tree to {}", commit_id, path_str))?;

    println!("Created new repo in {} with commit {} read into it", path_str, commit_id);
    Ok(new_repo)
}

/// Copy a tree from one repo into another
pub fn copy_tree<'src, 'dst>(
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

    // Convert to an index to do the checkout
    let mut index = git2::Index::new()
        .context("Creating in-memory index")?;
    index.read_tree(&tree)
        .with_context(|| format!("reading tree {} into index", tree.id()))?;

    let new_id = index.write_tree_to(dest)
        .with_context(|| format!("writing tree {} into index", tree.id()))?;
    let new_obj = dest.find_object(new_id, None)
        .with_context(|| format!("finding object {} that we just created", new_id))?;
    dest.checkout_tree(&new_obj, None)
        .with_context(|| format!("checking out {}", new_id))?;

    Ok(())
}
    

