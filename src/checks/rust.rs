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
use std::{fmt, fs};

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
#[derive(Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// A rust check
#[derive(Clone, Debug, PartialOrd, Ord, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(Deserialize)]
struct CargoToml {
    #[serde(default)]
    bin: Vec<Example>,
    #[serde(default)]
    example: Vec<Example>,
}

#[derive(Deserialize)]
struct Example {
    name: String,
}

impl RustCheck {
    pub fn execute(&self, repo: TempRepo, build_pool: &ThreadPool) -> anyhow::Result<Vec<String>> {
        let default_versions = vec!["stable".to_owned()];
        let versions = if self.version.is_empty() {
            default_versions
        } else {
            self.version.clone()
        };

        let head = repo.repo.head().context("getting HEAD")?.target().unwrap();
        let mut handles = vec![];
        for ver in versions {
            let fresh_repo = temp_repo(&repo.repo, head)
                .with_context(|| format!("creating temporary repo for {}", head))?;
            let mut path = fresh_repo.dir.path().to_path_buf();
            if let Some(ext) = self.working_dir.as_ref() {
                path.push(ext);
            }

            let toml_path = path.join("Cargo.toml");
            let toml_str = fs::read_to_string(&toml_path)
                .with_context(|| format!("reading {}", toml_path.to_string_lossy()))?;
            let toml: CargoToml = toml::from_str(&toml_str)
                .with_context(|| format!("parsing {}", toml_path.to_string_lossy()))?;

            let data = JobData {
                version: ver.clone(),
                commit: head,
            };

            let jobs = self.jobs.clone();
            handles.push(JobHandle::spawn(build_pool, data, move || {
                let _fresh_repo = fresh_repo; // pull repo into subthread so it doesn't drop() and delete our working dir

                let cargo = Cargo::new(&ver, &path);
                let c_ver = cargo.version_string()?;
                let r_ver = cargo.rustc_version_string()?;
                for job in &jobs {
                    match *job {
                        RustJob::Build => {
                            println!("Building {} ({} / {})", head, c_ver, r_ver);
                            cargo.build()?;
                        }
                        RustJob::Test => {
                            println!("Testing {} ({} / {})", head, c_ver, r_ver);
                            cargo.test()?;
                        }
                        RustJob::Examples => {
                            toml.example.par_iter().try_for_each(|ex| {
                                // Need a new cargo as the old one internally has stdout/err
                                // `File`s that cannot be shared across threads
                                let cargo = Cargo::new(&ver, &path);
                                println!(
                                    "Running example {} on {} ({} / {})",
                                    ex.name, head, c_ver, r_ver,
                                );
                                cargo.example(&ex.name)
                            })?;
                        }
                        RustJob::Fuzz { iters } => {
                            toml.bin.par_iter().try_for_each(|fuzz| {
                                // Need a new cargo as the old one internally has stdout/err
                                // `File`s that cannot be shared across threads
                                let cargo = Cargo::new(&ver, &path);
                                println!(
                                    "Fuzzing {} on {} ({} / {})",
                                    fuzz.name, head, c_ver, r_ver,
                                );
                                cargo.fuzz(&fuzz.name, iters)
                            })?;
                        }
                    }
                }
                Ok(())
            }));
        }

        println!("Spawned {} jobs", handles.len());
        let mut result = Ok(vec![]);
        for h in handles {
            let new_res = h.join().with_context(|| {
                format!(
                    "executing command on commit {} with cargo {}",
                    h.data.commit, h.data.version,
                )
            });
            match new_res {
                Ok(..) => { /* TODO */ }
                Err(e) => result = Err(e),
            }

            println!(
                "success on commit {} with cargo {}",
                h.data.commit, h.data.version,
            );
        }

        result
    }
}

struct JobData {
    version: String,
    commit: git2::Oid,
}
