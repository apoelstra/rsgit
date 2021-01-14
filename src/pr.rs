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

use std::collections::{HashMap, HashSet};
use std::collections::hash_map::Entry;
use git2::{Oid, Repository};

/// Pull request branch
pub struct PullRequest {
    /// Number of the PR on Github/Gitlab
    pub number: usize,
    /// Git ID of the tip of the PR branch
    pub id: Oid,
}

impl PullRequest {
    /// Scan through the commits in a PR branch, running some action on each one
    pub fn for_each_commit<'label, F: FnMut(Oid, usize, usize)>(
        &self,
        repo: &Repository,
        master_commits: &HashSet<Oid>,
        mut action: F,
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
            action(id, index, n_commits);
        }
    }
}

