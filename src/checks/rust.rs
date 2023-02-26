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

//! Checks for rust codebases

use anyhow::Context;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rayon::ThreadPool;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::{fmt, mem};
use tempfile::TempDir;

use crate::cargo::Cargo;
use crate::git::{temp_repo, TempRepo};
use crate::job::JobHandle;

fn default_rust_jobs() -> Vec<RustJob> {
    vec![RustJob::Build, RustJob::Test, RustJob::Examples]
}

fn default_fuzz_iters() -> usize {
    100_000
}

/// A rust-check job
#[derive(Copy, Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RustJob {
    Build,
    Examples,
    Test,
    Fuzz {
        #[serde(default = "default_fuzz_iters")]
        iters: usize,
    },
}

/// A single check (i.e. cargo invocation)
struct SingleCheck<'a, 'b, 'c> {
    cargo_ver: String,
    repo: &'a TempDir,
    path_ext: Option<&'b String>,
    job: RustJob,
    ext: &'c [String],
}

impl<'a, 'b, 'c> SingleCheck<'a, 'b, 'c> {
    fn new(
        cargo_ver: String,
        repo: &'a TempDir,
        path_ext: Option<&'b String>,
        job: RustJob,
        ext: &'c [String],
    ) -> Self {
        SingleCheck {
            cargo_ver: cargo_ver,
            repo: repo,
            path_ext: path_ext,
            job: job,
            ext: ext,
        }
    }

    fn notes_str(&self) -> String {
        match self.job {
            RustJob::Build => format!(
                "{} cargo build '--features={}'",
                self.cargo_ver,
                self.ext.join(" "),
            ),
            RustJob::Test => format!(
                "{} cargo test '--features={}'",
                self.cargo_ver,
                self.ext.join(" "),
            ),
            RustJob::Examples => {
                format!("{} cargo run '--example {}'", self.cargo_ver, self.ext[0],)
            }
            RustJob::Fuzz { iters } => format!(
                "{} cargo hfuzz run {} # iters {}",
                self.cargo_ver, self.ext[0], iters,
            ),
        }
    }

    fn run(
        self,
        head: git2::Oid,
        existing_notes: &[String],
        new_notes: &Mutex<Vec<String>>,
    ) -> anyhow::Result<()> {
        let my_note = self.notes_str();
        for note in existing_notes {
            // Already done
            if note == &my_note {
                return Ok(());
            }
        }

        // Need a new cargo as the old one internally has stdout/err
        // `File`s that cannot be shared across threads
        let cargo = Cargo::new(self.cargo_ver, self.repo, self.path_ext);
        let c_ver = cargo.version_string()?;
        let r_ver = cargo.rustc_version_string()?;
        let result = match self.job {
            RustJob::Build => {
                println!(
                    "Building {} (features {:?}) ({} / {})",
                    head, self.ext, c_ver, r_ver
                );
                cargo.build(&self.ext)
            }
            RustJob::Test => {
                println!(
                    "Testing {} (features {:?}) ({} / {})",
                    head, self.ext, c_ver, r_ver
                );
                cargo.test(&self.ext)
            }
            RustJob::Examples => {
                assert_eq!(self.ext.len(), 1);
                println!(
                    "Running example {} on {} ({} / {})",
                    &self.ext[0], head, c_ver, r_ver,
                );
                cargo.example(&self.ext[0])
            }
            RustJob::Fuzz { iters } => {
                assert_eq!(self.ext.len(), 1);
                println!(
                    "Fuzzing {} on {} ({} / {})",
                    &self.ext[0], head, c_ver, r_ver,
                );
                cargo.fuzz(&self.ext[0], iters)
            }
        }?;
        new_notes.lock().unwrap().push(my_note);
        Ok(result)
    }
}

/// A rust check
#[derive(Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct RustCheck {
    #[serde(default)]
    features: Vec<String>,
    #[serde(default, deserialize_with = "super::single_or_seq")]
    version: Vec<String>,
    #[serde(
        default = "default_rust_jobs",
        deserialize_with = "super::single_or_seq"
    )]
    jobs: Vec<RustJob>,
    #[serde(default)]
    only_tip: bool,
    #[serde(default)]
    working_dir: Option<String>,
}

impl fmt::Display for RustCheck {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{{ rust {:?} {:?} }}", self.features, self.jobs)
    }
}

impl RustCheck {
    pub fn execute(&self, repo: TempRepo, build_pool: &ThreadPool) -> anyhow::Result<Vec<String>> {
        let default_versions = vec!["stable".to_owned()];
        let versions = if self.version.is_empty() {
            default_versions
        } else {
            self.version.clone()
        };

        let mut feature_matrix = vec![vec![]];
        if !self.features.is_empty() {
            feature_matrix.push(self.features.clone());
        }
        for feat in &self.features {
            feature_matrix.push(vec![feat.clone()]);
        }

        let head = repo.repo.head().context("getting HEAD")?.target().unwrap();
        let existing_notes = repo
            .repo
            .find_note(Some("refs/notes/check-commit"), head)
            .ok()
            .as_ref()
            .and_then(|note| note.message())
            .map(|text| text.split('\n').map(|s| s.to_owned()).collect())
            .unwrap_or(vec![]);
        let existing_notes = Arc::new(existing_notes);

        let mut handles = vec![];
        for ver in versions {
            let fresh_repo = temp_repo(&repo.repo, head)
                .with_context(|| format!("creating temporary repo for {}", head))?;

            let data = JobData {
                version: ver.clone(),
                commit: head,
                new_notes: Arc::new(Mutex::new(vec![])),
            };

            let jobs = self.jobs.clone();
            let path_ext = self.working_dir.clone();
            let feature_matrix = feature_matrix.clone();
            let notes = existing_notes.clone();
            let new_notes = data.new_notes.clone();
            handles.push(JobHandle::spawn(build_pool, data, move || {
                let repo_dir = &fresh_repo.dir;

                let cargo = Cargo::new(ver.clone(), &repo_dir, path_ext.as_ref());
                cargo.pin_deps().context("pinning dependencies")?;

                let toml = cargo.toml()?;
                for job in &jobs {
                    match *job {
                        RustJob::Build | RustJob::Test => {
                            feature_matrix.par_iter().try_for_each(|feats| {
                                SingleCheck::new(
                                    ver.clone(),
                                    &repo_dir,
                                    path_ext.as_ref(),
                                    *job,
                                    feats,
                                )
                                .run(head, &*notes, &*new_notes)
                            })?;
                        }
                        RustJob::Examples => {
                            toml.example.par_iter().try_for_each(|ex| {
                                SingleCheck::new(
                                    ver.clone(),
                                    &repo_dir,
                                    path_ext.as_ref(),
                                    *job,
                                    &[ex.name.clone()],
                                )
                                .run(head, &*notes, &*new_notes)
                            })?;
                        }
                        RustJob::Fuzz { .. } => {
                            toml.bin.par_iter().try_for_each(|fuzz| {
                                SingleCheck::new(
                                    ver.clone(),
                                    &repo_dir,
                                    path_ext.as_ref(),
                                    *job,
                                    &[fuzz.name.clone()],
                                )
                                .run(head, &*notes, &*new_notes)
                            })?;
                        }
                    }
                }
                Ok(())
            }));
        }

        let mut result = Ok(vec![]);
        for h in handles {
            let new_res = h.join().with_context(|| {
                format!(
                    "executing command on commit {} with cargo {}",
                    h.data.commit, h.data.version,
                )
            });
            match new_res {
                Ok(()) => {
                    if let Ok(ref mut ret_notes) = result {
                        let new_notes =
                            mem::replace(&mut *h.data.new_notes.lock().unwrap(), vec![]);
                        ret_notes.extend(new_notes);
                    }
                }
                Err(e) => result = Err(e),
            }

            println!(
                "Completed all checks (commit {}, cargo {}",
                h.data.commit, h.data.version,
            );
        }

        result
    }
}

struct JobData {
    version: String,
    commit: git2::Oid,
    new_notes: Arc<Mutex<Vec<String>>>,
}
