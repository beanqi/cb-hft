use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownReason {
    UserRequested,
    FatalError,
    SessionDisconnected,
}

#[derive(Clone, Default)]
pub struct ShutdownSignal {
    reason: Arc<Mutex<Option<ShutdownReason>>>,
}

impl ShutdownSignal {
    pub fn request(&self, reason: ShutdownReason) {
        let mut guard = self.reason.lock().expect("shutdown mutex poisoned");
        if guard.is_none() {
            *guard = Some(reason);
        }
    }

    pub fn is_shutdown(&self) -> bool {
        self.reason().is_some()
    }

    pub fn reason(&self) -> Option<ShutdownReason> {
        *self.reason.lock().expect("shutdown mutex poisoned")
    }
}
