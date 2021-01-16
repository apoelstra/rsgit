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

use std::io::{self, BufRead, Read};
use std::path::Path;

use crate::job::exec_or_stderr;

/// Structure representing a cargo command
pub struct Cargo {
    exec: subprocess::Exec,
}

impl Cargo {
    /// Construct a new cargo instance
    pub fn new<P: AsRef<Path>>(version: &str, cwd: P) -> Cargo {
        Cargo {
            exec: subprocess::Exec::cmd("cargo")
                .arg(format!("+{}", version))
                .stdin(subprocess::NullFile)
                .cwd(cwd),
        }
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
        let exec = self
            .exec
            .clone()
            .arg("rustc")
            .arg("--")
            .arg("-V")
            .stdout(subprocess::Redirection::Pipe)
            .stderr(subprocess::NullFile);
        let invocation = exec.to_cmdline_lossy();
        let mut popen = exec
            .popen()
            .context("constructing cargo rustc -V process")?;
        let _ = popen.wait().context("waiting on cargo rustc -V")?;
        let bufread = io::BufReader::new(popen.stdout.take().unwrap());
        match bufread.lines().next() {
            Some(ver) => Ok(ver?),
            None => Err(anyhow::Error::msg(
                format!("no output from {}", invocation,),
            )),
        }
    }

    /// Tries to execute the `cargo build` command
    pub fn build(&self) -> anyhow::Result<()> {
        exec_or_stderr(self.exec.clone().arg("build"))
    }

    /// Tries to execute the `cargo build` command
    pub fn test(&self) -> anyhow::Result<()> {
        exec_or_stderr(self.exec.clone().arg("test"))
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
