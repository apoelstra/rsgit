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

//! Keeping track of processes

use anyhow::Context;
use rayon::ThreadPool;
use std::io::Read;
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

/// Handle to construct/spawn an async job
pub struct JobHandle<T> {
    pub data: T,
    rx: mpsc::Receiver<anyhow::Result<()>>,
    joined: AtomicBool,
}

impl<T> JobHandle<T> {
    /// Creates a new job and starts running it in the threadpool
    pub fn spawn<F>(pool: &ThreadPool, ext_data: T, f: F) -> Self
    where
        F: Fn() -> anyhow::Result<()> + Send + panic::UnwindSafe + 'static,
    {
        let (tx, rx) = mpsc::channel();
        pool.spawn(move || match panic::catch_unwind(f) {
            Ok(res) => tx.send(res).unwrap(),
            Err(_) => tx
                .send(Err(anyhow::Error::msg("a build job panicked")))
                .unwrap(),
        });
        JobHandle {
            data: ext_data,
            rx: rx,
            joined: AtomicBool::new(false),
        }
    }

    /// Waits for the job to complete
    pub fn join(&self) -> anyhow::Result<()> {
        self.joined.store(true, Ordering::SeqCst);
        self.rx.recv().unwrap()
    }
}

impl<T> Drop for JobHandle<T> {
    fn drop(&mut self) {
        if !self.joined.load(Ordering::SeqCst) {
            eprintln!("dropping jobhandle without receiving its result");
            eprintln!("{:?}", backtrace::Backtrace::new());
        }
    }
}

/// Helper function to try to execute a command, putting
/// stderr in the error return if it fails
pub fn exec_or_stderr(e: subprocess::Exec) -> anyhow::Result<()> {
    let invocation = e.to_cmdline_lossy();
    let mut popen = e
        .stdout(subprocess::NullFile)
        .stderr(subprocess::Redirection::Pipe)
        .popen()
        .with_context(|| format!("constructing Exec: {}", invocation))?;
    let fail_msg = match popen
        .wait()
        .with_context(|| format!("waiting: {}", invocation))?
    {
        subprocess::ExitStatus::Exited(0) => None,
        subprocess::ExitStatus::Exited(x) => Some(format!("exited with {}", x)),
        other => Some(format!("exited with {:?}", other)),
    };
    match fail_msg {
        Some(bad) => {
            let mut stderr = String::new();
            popen
                .stderr
                .as_mut()
                .unwrap()
                .read_to_string(&mut stderr)
                .with_context(|| format!("reading stderr from: {}", invocation))?;
            Err(anyhow::Error::msg(format!(
                "{}: {}\nstderr:\n{}",
                invocation, bad, stderr
            )))
        }
        None => Ok(()),
    }
}
