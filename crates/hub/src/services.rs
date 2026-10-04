#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Services {
    Interactive,
    Isolated,
}

impl Services {
    pub fn local<T>(self, operation: impl FnOnce() -> T) -> Option<T> {
        match self {
            Self::Interactive => Some(operation()),
            Self::Isolated => None,
        }
    }

    pub fn spawn(
        self,
        future: impl Future<Output = ()> + Send + 'static,
    ) -> Option<tokio::task::JoinHandle<()>> {
        self.local(|| tokio::spawn(future))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[tokio::test]
    async fn isolated_services_do_not_poll_external_work() {
        let called = Arc::new(AtomicBool::new(false));
        let observed = called.clone();
        if let Some(task) = Services::Isolated.spawn(async move {
            observed.store(true, Ordering::SeqCst);
        }) {
            task.await.unwrap();
        }
        assert!(!called.load(Ordering::SeqCst));
    }

    #[test]
    fn isolated_services_do_not_resolve_user_configuration() {
        let mut called = false;
        let result = Services::Isolated.local(|| {
            called = true;
            "/home/me/.claude"
        });
        assert!(!called);
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn interactive_services_complete_work() {
        let called = Arc::new(AtomicBool::new(false));
        let observed = called.clone();
        Services::Interactive
            .spawn(async move {
                observed.store(true, Ordering::SeqCst);
            })
            .unwrap()
            .await
            .unwrap();
        assert!(called.load(Ordering::SeqCst));
    }
}
