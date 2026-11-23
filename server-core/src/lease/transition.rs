#![deny(clippy::disallowed_methods)]

use std::time::Instant;

use hebbian_contracts::LeaseHolder;

pub(super) enum LeasePhase<Pending> {
    Active,
    Transferring,
    Interrupted,
    ExpiredManual(Pending),
}

pub(super) enum LeaseEffect<'a, Pending> {
    None,
    StopExpired(ExpiryPolicy),
    CheckSettling(&'a mut Pending),
}

#[derive(Clone, Copy)]
pub(super) enum ExpiryPolicy {
    KeepFenced,
    ReleaseWhenSettled,
}

#[derive(Clone, Copy)]
pub(super) enum Settling {
    Pending,
    Confirmed,
    Unavailable,
}

pub(super) enum SettlingDecision {
    KeepWaiting,
    Release,
    Fence,
}

#[derive(Clone, Copy)]
pub(super) enum TakeoverMode {
    Recover,
    Replace,
}

#[derive(Clone, Copy)]
pub(super) enum TakeoverResult {
    Settled,
    Failed,
}

impl<Pending> LeasePhase<Pending> {
    pub(super) fn on_time(
        &mut self,
        holder: LeaseHolder,
        expires_at: Instant,
        now: Instant,
    ) -> LeaseEffect<'_, Pending> {
        match self {
            Self::Active if expires_at <= now => {
                // SAFETY: Revoke authority even when the controller cannot accept a stop.
                *self = Self::Interrupted;
                LeaseEffect::StopExpired(match holder {
                    LeaseHolder::Agent => ExpiryPolicy::KeepFenced,
                    LeaseHolder::Manual => ExpiryPolicy::ReleaseWhenSettled,
                })
            }
            Self::ExpiredManual(pending) => LeaseEffect::CheckSettling(pending),
            Self::Active | Self::Transferring | Self::Interrupted => LeaseEffect::None,
        }
    }

    pub(super) fn can_start_takeover(&self, mode: TakeoverMode) -> bool {
        match self {
            Self::Active => matches!(mode, TakeoverMode::Replace),
            Self::Transferring => false,
            Self::Interrupted | Self::ExpiredManual(_) => true,
        }
    }

    pub(super) fn finish_takeover(&mut self, result: TakeoverResult) -> Option<TakeoverResult> {
        match self {
            Self::Transferring => {
                *self = match result {
                    TakeoverResult::Settled => Self::Active,
                    TakeoverResult::Failed => Self::Interrupted,
                };
                Some(result)
            }
            Self::Active | Self::Interrupted | Self::ExpiredManual(_) => None,
        }
    }
}

pub(super) fn expired_manual_stop(progress: Settling) -> SettlingDecision {
    match progress {
        Settling::Pending => SettlingDecision::KeepWaiting,
        Settling::Confirmed => SettlingDecision::Release,
        Settling::Unavailable => SettlingDecision::Fence,
    }
}

pub(super) enum TakeoverPoll {
    Complete,
    Wait,
    TimedOut,
}

pub(super) fn takeover_poll(all_settled: bool, now: Instant, deadline: Instant) -> TakeoverPoll {
    if all_settled {
        TakeoverPoll::Complete
    } else if now < deadline {
        TakeoverPoll::Wait
    } else {
        TakeoverPoll::TimedOut
    }
}
