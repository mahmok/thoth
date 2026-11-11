use alloc::collections::VecDeque;
use alloc::rc::Rc;
use core::cell::RefCell;

use crate::llm::provider::Provider;
use crate::models::refresh::{Event as RefreshEvent, Worker as RefreshWorker};

#[derive(Default, Debug)]
/// A refresh worker that replays a script.
pub struct Worker {
    events: RefCell<VecDeque<RefreshEvent>>,
    started: RefCell<Vec<Vec<Provider>>>,
}

impl Worker {
    /// Queues `event` as the running refresh would report it.
    #[inline]
    pub fn event(&self, event: RefreshEvent) {
        self.events.borrow_mut().push_back(event);
    }

    #[must_use]
    #[inline]
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    #[inline]
    pub fn started(&self) -> Vec<Vec<Provider>> {
        self.started.borrow().clone()
    }
}

impl RefreshWorker for Rc<Worker> {
    #[inline]
    fn start(&self, providers: Vec<Provider>) {
        self.started.borrow_mut().push(providers);
    }

    #[inline]
    fn try_next(&self) -> Option<RefreshEvent> {
        self.events.borrow_mut().pop_front()
    }
}
