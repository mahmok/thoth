use alloc::collections::VecDeque;
use alloc::rc::Rc;
use core::cell::RefCell;

use crate::auth::flow::LoginEvent;
use crate::auth::session::LoginWorker;
use crate::llm::provider::Provider;

#[derive(Default, Debug)]
/// A login worker that replays a script.
pub struct Worker {
    answers: RefCell<Vec<String>>,
    cancelled: RefCell<usize>,
    events: RefCell<VecDeque<LoginEvent>>,
    started: RefCell<Vec<Provider>>,
}

impl Worker {
    #[inline]
    pub fn answers(&self) -> Vec<String> {
        self.answers.borrow().clone()
    }

    #[inline]
    pub fn cancelled(&self) -> usize {
        *self.cancelled.borrow()
    }

    /// Queues `event` as the running login would report it.
    #[inline]
    pub fn event(&self, event: LoginEvent) {
        self.events.borrow_mut().push_back(event);
    }

    #[must_use]
    #[inline]
    pub fn new() -> Rc<Self> {
        Rc::new(Self::default())
    }

    #[inline]
    pub fn started(&self) -> Vec<Provider> {
        self.started.borrow().clone()
    }
}

impl LoginWorker for Rc<Worker> {
    #[inline]
    fn answer(&self, secret: String) {
        self.answers.borrow_mut().push(secret);
    }

    #[inline]
    fn cancel(&self) {
        {
            let mut cancelled = self.cancelled.borrow_mut();
            *cancelled = cancelled.saturating_add(1);
        }
    }

    #[inline]
    fn start(&self, provider: Provider) {
        self.started.borrow_mut().push(provider);
    }

    #[inline]
    fn try_next(&self) -> Option<LoginEvent> {
        self.events.borrow_mut().pop_front()
    }
}
