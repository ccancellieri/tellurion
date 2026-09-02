use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use thiserror::Error;

/// Limits that apply to a bounded group of origin requests.
#[derive(Debug, Clone, Copy)]
pub struct BudgetLimits {
    pub requests: u32,
    pub bytes: u64,
    pub deadline: Duration,
    pub concurrent: u32,
}

/// A thread-safe, fail-closed request and byte budget.
#[derive(Debug)]
pub struct Budget {
    limits: BudgetLimits,
    deadline: Instant,
    state: Mutex<BudgetState>,
}

#[derive(Debug, Default)]
struct BudgetState {
    requests: u32,
    charged_bytes: u64,
    reserved_bytes: u64,
    concurrent: u32,
    invalidated: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Error)]
pub enum BudgetErrorKind {
    #[error("request limit reached")]
    RequestLimit,
    #[error("byte limit reached")]
    ByteLimit,
    #[error("deadline reached")]
    Deadline,
    #[error("concurrent operation limit reached")]
    ConcurrentLimit,
    #[error("source is invalidated")]
    Invalidated,
    #[error("budget state is unavailable")]
    Poisoned,
}

#[derive(Debug, Clone, Copy, Error)]
#[error("{kind}")]
pub struct BudgetError {
    kind: BudgetErrorKind,
}

impl BudgetError {
    pub fn kind(self) -> BudgetErrorKind {
        self.kind
    }
}

impl Budget {
    pub fn new(limits: BudgetLimits) -> Self {
        Self {
            limits,
            deadline: Instant::now() + limits.deadline,
            state: Mutex::new(BudgetState::default()),
        }
    }

    /// Reserves the maximum bytes before an outbound request can begin.
    pub fn reserve(&self, maximum_bytes: u64) -> Result<BudgetReservation<'_>, BudgetError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| BudgetError::new(BudgetErrorKind::Poisoned))?;
        if state.invalidated {
            return Err(BudgetError::new(BudgetErrorKind::Invalidated));
        }
        if Instant::now() >= self.deadline {
            return Err(BudgetError::new(BudgetErrorKind::Deadline));
        }
        if state.concurrent >= self.limits.concurrent {
            return Err(BudgetError::new(BudgetErrorKind::ConcurrentLimit));
        }
        if state.requests >= self.limits.requests {
            return Err(BudgetError::new(BudgetErrorKind::RequestLimit));
        }
        if state
            .charged_bytes
            .saturating_add(state.reserved_bytes)
            .saturating_add(maximum_bytes)
            > self.limits.bytes
        {
            return Err(BudgetError::new(BudgetErrorKind::ByteLimit));
        }

        state.requests += 1;
        state.concurrent += 1;
        state.reserved_bytes += maximum_bytes;
        Ok(BudgetReservation {
            budget: self,
            reserved_bytes: maximum_bytes,
            completed: false,
        })
    }

    pub fn invalidate(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.invalidated = true;
        }
    }

    pub fn is_invalidated(&self) -> bool {
        self.state.lock().map_or(true, |state| state.invalidated)
    }

    pub(crate) fn remaining(&self) -> Result<Duration, BudgetError> {
        let _state = self
            .state
            .lock()
            .map_err(|_| BudgetError::new(BudgetErrorKind::Poisoned))?;
        self.deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| BudgetError::new(BudgetErrorKind::Deadline))
    }

    fn finish(&self, reserved_bytes: u64, actual_bytes: u64) -> Result<(), BudgetError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| BudgetError::new(BudgetErrorKind::Poisoned))?;
        state.reserved_bytes -= reserved_bytes;
        state.concurrent -= 1;
        if state.charged_bytes.saturating_add(actual_bytes) > self.limits.bytes {
            state.invalidated = true;
            return Err(BudgetError::new(BudgetErrorKind::ByteLimit));
        }
        state.charged_bytes += actual_bytes;
        Ok(())
    }

    fn abandon(&self, reserved_bytes: u64) {
        if let Ok(mut state) = self.state.lock() {
            state.reserved_bytes = state.reserved_bytes.saturating_sub(reserved_bytes);
            state.concurrent = state.concurrent.saturating_sub(1);
        }
    }

    #[cfg(test)]
    pub(crate) fn poison_for_test(&self) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = self.state.lock().expect("fresh budget mutex");
            panic!("poison test mutex");
        }));
    }
}

impl BudgetError {
    const fn new(kind: BudgetErrorKind) -> Self {
        Self { kind }
    }
}

/// An in-flight request reservation. Dropping it releases unused capacity.
#[derive(Debug)]
pub struct BudgetReservation<'a> {
    budget: &'a Budget,
    reserved_bytes: u64,
    completed: bool,
}

impl BudgetReservation<'_> {
    /// Charges the bytes actually read and releases the in-flight slot.
    pub fn finish(mut self, actual_bytes: u64) -> Result<(), BudgetError> {
        self.completed = true;
        self.budget.finish(self.reserved_bytes, actual_bytes)
    }
}

impl Drop for BudgetReservation<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.budget.abandon(self.reserved_bytes);
        }
    }
}
