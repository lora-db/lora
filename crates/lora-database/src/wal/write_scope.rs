use anyhow::Result;
use lora_store::MutationEvent;
use lora_wal::{Lsn, WalBufferedCommitError, WalRecorder};

/// Commit LSN and events of a write committed while a change feed captures.
pub(crate) type CapturedCommit = (Lsn, Vec<MutationEvent>);

/// Policy for a failed write while a WAL scope is armed.
#[derive(Debug, Clone, Copy)]
pub(crate) enum WalAbortPolicy {
    /// Abort the pending WAL batch, but do not quarantine the live graph.
    AbortOnly,
    /// If the recorder observed mutations before the failure, poison it so
    /// callers restart from durable state instead of serving a possibly
    /// divergent live graph.
    PoisonIfMutated(&'static str),
}

/// Armed WAL scope for database paths that mutate the live graph directly.
///
/// Hosts open this immediately before running a mutating closure, then call
/// [`finish`] with that closure's result. The WAL crate owns recorder mechanics;
/// this scope owns database policy: whether a failed partial write poisons the
/// live graph and when managed snapshot accounting should be triggered.
///
/// [`finish`]: Self::finish
pub(crate) struct WalWriteScope<'a> {
    recorder: &'a WalRecorder,
    abort_policy: WalAbortPolicy,
}

impl<'a> WalWriteScope<'a> {
    pub(crate) fn arm(recorder: &'a WalRecorder, abort_policy: WalAbortPolicy) -> Result<Self> {
        recorder.arm().map_err(WalBufferedCommitError::Arm)?;
        Ok(Self {
            recorder,
            abort_policy,
        })
    }

    /// Finish the armed WAL scope.
    ///
    /// Returns `true` when a WAL commit record was written and flushed. The
    /// caller can use that to trigger managed snapshot accounting exactly once.
    ///
    /// With `capture`, a written commit also hands back its commit LSN and
    /// events for the change feed.
    pub(crate) fn finish<R>(
        self,
        result: &Result<R>,
        capture: bool,
    ) -> Result<(bool, Option<CapturedCommit>)> {
        let outcome = match result {
            Ok(_) if capture => {
                let captured = self.recorder.commit_capture()?;
                (captured.is_some(), captured)
            }
            Ok(_) => (self.recorder.commit()?.wrote(), None),
            Err(_) => {
                abort_armed(self.recorder, self.abort_policy)?;
                (false, None)
            }
        };
        ensure_wal_not_poisoned(self.recorder)?;
        Ok(outcome)
    }
}

pub(crate) fn ensure_wal_not_poisoned(recorder: &WalRecorder) -> Result<()> {
    if let Some(reason) = recorder.poisoned_reason() {
        return Err(WalBufferedCommitError::Poisoned(reason).into());
    }
    Ok(())
}

pub(crate) fn ensure_wal_query_can_start(recorder: &WalRecorder) -> Result<()> {
    if let Some(reason) = recorder.poisoned_reason() {
        return Err(WalBufferedCommitError::Poisoned(reason).into());
    }
    Ok(())
}

fn abort_armed(recorder: &WalRecorder, policy: WalAbortPolicy) -> Result<()> {
    let aborted_after_mutation = matches!(recorder.abort(), Ok(true));
    if aborted_after_mutation {
        if let WalAbortPolicy::PoisonIfMutated(reason) = policy {
            recorder.poison(reason);
        }
    }
    Ok(())
}
