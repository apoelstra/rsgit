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

//! Utilities for handling a cargo instance

use anyhow::Context;
use serde::Deserialize;
use tempfile::TempDir;

use std::fs;
use std::io::{self, BufRead, Read};
use std::path::PathBuf;

use crate::git::RepoRef;
use crate::job::exec_or_stderr;

/// Structure representing a cargo command
pub struct Cargo<'a> {
    exec: subprocess::Exec,
    cwd: PathBuf,
    version: String,
    _ref: RepoRef<'a>,
}

impl<'a> Cargo<'a> {
    /// Construct a new cargo instance
    pub fn new(version: String, tmp_dir: &'a TempDir, cwd_ext: Option<&String>) -> Self {
        let mut cwd = tmp_dir.path().to_path_buf();
        if let Some(s) = cwd_ext {
            cwd.push(s);
        }

        Cargo {
            exec: subprocess::Exec::cmd("cargo")
                .arg(format!("+{}", version))
                .stdin(subprocess::NullFile)
                .cwd(&cwd),
            cwd: cwd,
            version: version,
            _ref: tmp_dir.into(),
        }
    }

    /// Gets a parsed version of the toml file
    pub fn toml(&self) -> anyhow::Result<CargoToml> {
        let toml_path = self.cwd.join("Cargo.toml");
        let toml_str = fs::read_to_string(&toml_path)
            .with_context(|| format!("reading {}", toml_path.to_string_lossy()))?;
        let toml: CargoToml = toml::from_str(&toml_str)
            .with_context(|| format!("parsing {}", toml_path.to_string_lossy()))?;
        Ok(toml)
    }

    /// Gets the version string of the cargo instance
    pub fn version_string(&self) -> anyhow::Result<String> {
        let mut popen = self
            .exec
            .clone()
            .arg("-V")
            .stdout(subprocess::Redirection::Pipe)
            .stderr(subprocess::Redirection::Pipe)
            .popen()
            .context("constructing cargo -V process")?;
        let exit_status = popen.wait().context("waiting on cargo -V")?;
        let bufread = io::BufReader::new(popen.stdout.take().unwrap());
        match bufread.lines().next() {
            Some(ver) => Ok(ver?),
            None => {
                let mut stderr = String::new();
                popen.stderr.as_mut().unwrap().read_to_string(&mut stderr)?;
                Err(anyhow::Error::msg(format!(
                    "no cargo -V output. status {:?}, stderr {:?}",
                    exit_status, stderr,
                )))
            }
        }
    }

    /// Gets the version string of the cargo instance
    pub fn rustc_version_string(&self) -> anyhow::Result<String> {
        let exec = subprocess::Exec::cmd("rustc")
            .arg(format!("+{}", self.version))
            .stdin(subprocess::NullFile)
            .arg("-V")
            .stdout(subprocess::Redirection::Pipe)
            .stderr(subprocess::Redirection::Pipe);
        let invocation = exec.to_cmdline_lossy();
        let mut popen = exec.popen().context("constructing rustc -V process")?;
        let exit_status = popen.wait().context("waiting on rustc -V")?;
        let bufread = io::BufReader::new(popen.stdout.take().unwrap());
        match bufread.lines().next() {
            Some(ver) => Ok(ver?),
            None => {
                let mut stderr = String::new();
                popen.stderr.as_mut().unwrap().read_to_string(&mut stderr)?;
                Err(anyhow::Error::msg(format!(
                    "no {} output. status {:?}, stderr {:?}",
                    invocation, exit_status, stderr,
                )))
            }
        }
    }

    fn pin_dep(&self, dep: &str, version: &str) {
        let exec = self.exec.clone().arg("update");
        println!("Version {}: pinning {} to {}. ", self.version, dep, version);
        if let Err(e) = exec_or_stderr(exec.arg("-p").arg(dep).arg("--precise").arg(version)) {
            println!(
                "failed) Version {}: pinning {} to {}. Error {}",
                self.version, dep, version, e
            );
        }
    }

    /// Tries to execute the `cargo build` command
    pub fn pin_deps(&self) -> anyhow::Result<()> {
        // Gate everything on generating the lockfile. Sometimes we
        // can't, e.g. if the project has `cargo vendor`ed a git repo.
        // In this case we can't pin deps anyway so don't try.
        if exec_or_stderr(self.exec.clone().arg("generate-lockfile")).is_ok() {
            exec_or_stderr(self.exec.clone().arg("update"))?;
            if &self.version[..] < "1.31.0" {
                // Also don't report failure on any of these, since we don't
                // know which deps are actually used
                self.pin_dep("byteorder", "1.3.4");
                self.pin_dep("cc", "1.0.41");
                self.pin_dep("serde_json", "1.0.39");
                self.pin_dep("serde", "1.0.98");
                self.pin_dep("serde_derive", "1.0.98");
            }
        }
        Ok(())
    }

    /// Tries to execute the `cargo build` command
    pub fn build(&self, features: &[String]) -> anyhow::Result<()> {
        exec_or_stderr(
            self.exec
                .clone()
                .arg("build")
                .arg(format!("--features={}", features.join(" "))),
        )
    }

    /// Tries to execute the `cargo test` command
    pub fn test(&self, features: &[String]) -> anyhow::Result<()> {
        exec_or_stderr(
            self.exec
                .clone()
                .arg("test")
                .arg(format!("--features={}", features.join(" "))),
        )
    }

    /// Tries to execute the `cargo run --example` command
    pub fn example(&self, ex: &str) -> anyhow::Result<()> {
        exec_or_stderr(self.exec.clone().arg("run").arg("--example").arg(ex))
    }

    /// Tries to execute the `cargo run --example` command
    pub fn fuzz(&self, bin: &str, iters: usize) -> anyhow::Result<()> {
        let exec = self
            .exec
            .clone()
            .env("HFUZZ_BUILD_ARGS", "--features honggfuzz_fuzz")
            .env(
                "HFUZZ_RUN_ARGS",
                format!("--exit_upon_crash -v -N{}", iters),
            )
            .arg("hfuzz")
            .arg("run")
            .arg(bin);
        exec_or_stderr(exec)
    }
}

#[derive(Deserialize)]
pub struct CargoToml {
    #[serde(default)]
    pub bin: Vec<Example>,
    #[serde(default)]
    pub example: Vec<Example>,
}

#[derive(Deserialize)]
pub struct Example {
    pub name: String,
}
