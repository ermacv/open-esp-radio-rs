//! One operation-wide cooperative work counter and deadline.
use blobray_domain::*;

pub struct RunContext<'a> {
    environment: &'a dyn RunEnvironment,
    started_ms: u64,
    deadline_ms: u64,
    limit: u64,
    progress: RunProgress,
    failure: Option<Error>,
}
impl<'a> RunContext<'a> {
    pub fn new(
        environment: &'a dyn RunEnvironment,
        started_ms: u64,
        deadline_ms: u64,
        budget: &ResourceBudget,
        prior: Option<RunProgress>,
    ) -> Result<Self> {
        budget.validate()?;
        let progress = prior.unwrap_or_default();
        let limit = budget.max_work_units.unwrap();
        if progress.work_used > limit || progress.stop.is_some() || deadline_ms < started_ms {
            return Err(Error::new(
                ErrorCode::Integrity,
                "invalid resumed execution context",
            ));
        }
        Ok(Self {
            environment,
            started_ms,
            deadline_ms,
            limit,
            progress,
            failure: None,
        })
    }
    pub fn snapshot(&self) -> RunProgress {
        self.progress
    }
    fn account_time(&mut self) {
        let elapsed = self.environment.now_ms().saturating_sub(self.started_ms);
        self.progress.phases.add(
            self.progress.position.phase,
            elapsed.saturating_sub(self.progress.elapsed_ms),
            0,
        );
        self.progress.elapsed_ms = elapsed;
    }
    pub fn flush(&mut self) -> Result<()> {
        self.account_time();
        let result = self.environment.observe(&self.progress);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
}
impl RunControl for RunContext<'_> {
    fn measure(&mut self, metric: WorkMetric, amount: u64) {
        self.progress.measurements.add(metric, amount);
    }
    fn progress(&self) -> Option<RunProgress> {
        Some(self.snapshot())
    }
    fn temporary_storage(&mut self, observation: TemporaryUsage) {
        self.progress.temporary_storage = Some(observation);
    }
    fn working_memory(&mut self, mut observation: WorkingMemoryObservation) {
        if let Some(prior) = self.progress.working_memory {
            observation.peak_reserved_bytes = observation
                .peak_reserved_bytes
                .max(prior.peak_reserved_bytes);
        }
        self.progress.working_memory = Some(observation);
    }
    fn position(&self) -> RunPosition {
        self.progress.position
    }
    fn set_position(&mut self, position: RunPosition) {
        if self.progress.position.phase != position.phase {
            self.account_time();
        }
        self.progress.position = position;
    }
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let now = self.environment.now_ms();
        self.account_time();
        self.progress.sequence = self.progress.sequence.saturating_add(1);
        let stop = if self.environment.cancelled() {
            Some((StopReason::Cancelled, ErrorCode::Cancelled))
        } else if now >= self.deadline_ms {
            Some((StopReason::Deadline, ErrorCode::TimedOut))
        } else if units > self.limit - self.progress.work_used {
            Some((StopReason::Work, ErrorCode::ResourceLimited))
        } else {
            None
        };
        if let Some((reason, code)) = stop {
            self.progress.stop = Some(ControlStop {
                reason,
                requested_units: (reason == StopReason::Work).then_some(units),
                limit: (reason == StopReason::Work).then_some(self.limit),
            });
            // The fixed-size cause is retained even if the observation channel fails.
            let error = Error::new(
                code,
                match reason {
                    StopReason::Work => "work budget exhausted",
                    StopReason::Deadline => "run deadline exceeded",
                    StopReason::Cancelled => "run cancelled",
                },
            );
            self.failure = Some(error.clone());
            return Err(error);
        }
        self.progress.work_used += units;
        self.progress
            .phases
            .add(self.progress.position.phase, 0, units);
        let result = self.environment.observe(&self.progress);
        if let Err(error) = &result {
            self.failure = Some(error.clone());
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    struct Environment {
        time: Cell<u64>,
        cancelled: Cell<bool>,
        fail_observer: bool,
    }
    impl RunEnvironment for Environment {
        fn now_ms(&self) -> u64 {
            self.time.get()
        }
        fn cancelled(&self) -> bool {
            self.cancelled.get()
        }
        fn observe(&self, _: &RunProgress) -> Result<()> {
            if self.fail_observer {
                Err(Error::new(ErrorCode::DiagnosticChannel, "fixture"))
            } else {
                Ok(())
            }
        }
    }
    fn environment() -> Environment {
        Environment {
            time: Cell::new(100),
            cancelled: Cell::new(false),
            fail_observer: false,
        }
    }
    #[test]
    fn fixed_measurements_and_phase_costs_survive_worker_handoff() {
        let env = environment();
        let budget = ResourceBudget::default();
        let mut worker = RunContext::new(&env, 100, 1000, &budget, None).unwrap();
        worker.phase(RunPhase::PrepareObject).unwrap();
        worker.measure(WorkMetric::ObjectsPrepared, 1);
        worker.checkpoint(7).unwrap();
        env.time.set(120);
        worker.phase(RunPhase::AnalyzeFunction).unwrap();
        worker.checkpoint(3).unwrap();
        env.time.set(150);
        worker.flush().unwrap();
        let mut coordinator =
            RunContext::new(&env, 100, 1000, &budget, Some(worker.snapshot())).unwrap();
        coordinator.phase(RunPhase::Retain).unwrap();
        coordinator.checkpoint(5).unwrap();
        env.time.set(160);
        coordinator.flush().unwrap();
        let p = coordinator.snapshot();
        assert_eq!(p.work_used, 15);
        assert_eq!(p.measurements.objects_prepared, 1);
        assert_eq!(
            p.phases.prepare_object,
            PhaseCost {
                elapsed_ms: 20,
                work_units: 7
            }
        );
        assert_eq!(
            p.phases.analyze_function,
            PhaseCost {
                elapsed_ms: 30,
                work_units: 3
            }
        );
        assert_eq!(
            p.phases.retain,
            PhaseCost {
                elapsed_ms: 10,
                work_units: 5
            }
        );
        assert_eq!(p.elapsed_ms, 60);
    }
    #[test]
    fn exact_budget_boundary_and_overflow_are_sticky() {
        let env = environment();
        let budget = ResourceBudget {
            max_work_units: Some(7),
            ..Default::default()
        };
        let mut context = RunContext::new(&env, 100, 1000, &budget, None).unwrap();
        context.checkpoint(7).unwrap();
        assert_eq!(
            context.checkpoint(u64::MAX).unwrap_err().code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(context.snapshot().work_used, 7);
        assert_eq!(
            context.snapshot().stop.unwrap().requested_units,
            Some(u64::MAX)
        );
        assert_eq!(
            context.checkpoint(0).unwrap_err().code,
            ErrorCode::ResourceLimited
        );
    }
    #[test]
    fn handoff_preserves_work_and_original_deadline() {
        let env = environment();
        let budget = ResourceBudget {
            max_work_units: Some(5),
            ..Default::default()
        };
        let mut worker = RunContext::new(&env, 100, 200, &budget, None).unwrap();
        worker.checkpoint(4).unwrap();
        let mut coordinator =
            RunContext::new(&env, 100, 200, &budget, Some(worker.snapshot())).unwrap();
        assert_eq!(
            coordinator.checkpoint(2).unwrap_err().code,
            ErrorCode::ResourceLimited
        );
        let mut coordinator =
            RunContext::new(&env, 100, 200, &budget, Some(worker.snapshot())).unwrap();
        env.time.set(200);
        assert_eq!(
            coordinator.checkpoint(0).unwrap_err().code,
            ErrorCode::TimedOut
        );
        assert_eq!(coordinator.snapshot().elapsed_ms, 100);
    }
    #[test]
    fn cancellation_and_observer_failure_are_distinct() {
        let env = environment();
        let mut context =
            RunContext::new(&env, 100, 200, &ResourceBudget::default(), None).unwrap();
        env.cancelled.set(true);
        assert_eq!(
            context.checkpoint(1).unwrap_err().code,
            ErrorCode::Cancelled
        );
        assert_eq!(context.snapshot().work_used, 0);
        let mut env = environment();
        env.fail_observer = true;
        let mut context =
            RunContext::new(&env, 100, 200, &ResourceBudget::default(), None).unwrap();
        assert_eq!(
            context.checkpoint(1).unwrap_err().code,
            ErrorCode::DiagnosticChannel
        );
        assert_eq!(
            context.checkpoint(1).unwrap_err().code,
            ErrorCode::DiagnosticChannel
        );
        assert_eq!(context.snapshot().work_used, 1);
    }
}
