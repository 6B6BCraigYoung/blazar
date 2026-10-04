#[derive(Clone, Copy, Default)]
pub struct JobPoll {
    known: bool,
    running: bool,
    failed: bool,
}

impl JobPoll {
    pub fn observe(&mut self, result: Result<bool, ()>) -> bool {
        let was_running = self.running;
        match result {
            Ok(running) => {
                self.known = true;
                self.running = running;
                self.failed = false;
                was_running && !running
            }
            Err(()) => {
                self.failed = true;
                false
            }
        }
    }

    pub fn started(&mut self) {
        self.known = true;
        self.running = true;
        self.failed = false;
    }

    pub fn start_failed(&mut self) {
        self.started();
        self.failed = true;
    }

    pub fn busy(self) -> bool {
        !self.known || self.running || self.failed
    }

    pub fn should_poll(self) -> bool {
        self.busy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_job_keeps_polling_and_stays_busy_after_transient_failure() {
        let mut poll = JobPoll::default();
        assert!(!poll.observe(Ok(true)));
        assert!(!poll.observe(Err(())));
        assert!(poll.busy());
        assert!(poll.should_poll());
        assert!(poll.observe(Ok(false)));
        assert!(!poll.busy());
        assert!(!poll.should_poll());
        assert!(!poll.observe(Ok(false)));
    }

    #[test]
    fn an_initial_job_status_error_is_retried_without_unlocking_connect_actions() {
        let mut poll = JobPoll::default();
        assert!(poll.busy());
        poll.observe(Err(()));
        assert!(poll.busy());
        assert!(poll.should_poll());
        poll.observe(Ok(false));
        assert!(!poll.busy());
        assert!(!poll.should_poll());
    }

    #[test]
    fn a_started_job_is_busy_before_the_first_poll_returns() {
        let mut poll = JobPoll::default();
        poll.observe(Ok(false));
        poll.started();
        assert!(poll.busy());
        assert!(poll.should_poll());
        assert!(poll.observe(Ok(false)));
    }

    #[test]
    fn an_uncertain_start_refreshes_app_status_after_confirming_the_job_is_terminal() {
        let mut poll = JobPoll::default();
        poll.observe(Ok(false));
        poll.start_failed();
        assert!(poll.busy());
        assert!(poll.should_poll());
        assert!(poll.observe(Ok(false)));
        assert!(!poll.busy());
    }
}
