//! Removals that run in a process of their own.
//!
//! `git worktree remove` walks a whole working tree looking for uncommitted work and then
//! deletes it, which is seconds on a repository of any size. Doing that inside the picker's
//! loop freezes it; doing it on a thread unfreezes it but keeps the process alive until the
//! deletion finishes, which is the one thing the user wants to stop doing once a checkout is
//! condemned. So it goes somewhere else entirely — see
//! `docs/adr/0014-removing-outlives-the-picker.md`.

use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use anyhow::{anyhow, Context, Result};

use crate::domain::removal;
use crate::port::{RemovalOutcome, RemovalPort, RunningRemoval};

/// Starts each removal as `herdr-worktree-nav remove …`, in a session of its own.
pub struct DetachedRemovals;

impl RemovalPort for DetachedRemovals {
    fn start(
        &self,
        repo_root: &str,
        checkout_path: &str,
        label: &str,
    ) -> Result<Box<dyn RunningRemoval>> {
        // This binary rather than `git` directly: the child has to reach herdr to report
        // itself, and doing that through the same ports the picker uses is what keeps the
        // words and the socket handling in one place.
        let exe = std::env::current_exe()
            .context("finding this binary again, to run the removal in a process of its own")?;

        let mut command = Command::new(exe);
        command
            .arg("remove")
            .arg(repo_root)
            .arg(checkout_path)
            .arg(label)
            .stdin(Stdio::null())
            // The report line comes back this way while the picker is still up to read it.
            .stdout(Stdio::piped())
            // Whatever the child could not even try is worth having when nothing readable
            // arrives on stdout.
            .stderr(Stdio::piped());

        // SAFETY: `setsid` is a bare syscall and async-signal-safe, which is the bar for
        // anything running between fork and exec. It is also the whole point: herdr kills a
        // closed pane's process group, and a new session is out of reach of that.
        unsafe {
            command.pre_exec(|| match libc::setsid() {
                -1 => Err(std::io::Error::last_os_error()),
                _ => Ok(()),
            });
        }

        let child = command
            .spawn()
            .with_context(|| format!("starting the removal of {checkout_path}"))?;
        Ok(Box::new(Detached {
            child,
            label: label.to_string(),
        }))
    }
}

struct Detached {
    child: Child,
    label: String,
}

impl RunningRemoval for Detached {
    fn wait(self: Box<Self>) -> Result<RemovalOutcome> {
        let label = self.label;
        let output = self
            .child
            .wait_with_output()
            .with_context(|| format!("waiting for the removal of {label}"))?;

        let reported = String::from_utf8_lossy(&output.stdout);
        if let Some(outcome) = reported.lines().find_map(removal::parse_report) {
            return Ok(outcome);
        }

        // Nothing readable came back. The removal may well have happened — the toast is
        // its own report and does not come through here — so this says what this side
        // knows and not what it guesses.
        let stderr = String::from_utf8_lossy(&output.stderr);
        let said = stderr.trim();
        Err(match said.is_empty() {
            true => anyhow!("the removal of {label} ended without saying what happened"),
            false => anyhow!("the removal of {label} ended without saying what happened: {said}"),
        })
    }
}
