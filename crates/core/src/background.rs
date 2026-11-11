use core::future::Future;
use core::mem;
use std::sync::Mutex;

use crate::sync::MutexExt as _;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;
#[derive(Debug)]

/// Work in flight and its events.
struct Running<E> {
    events: UnboundedReceiver<E>,
    task: JoinHandle<()>,
}

#[derive(Debug)]
/// One job at a time, polled for events.
pub struct Background<E> {
    running: Mutex<Option<Running<E>>>,
}

impl<E> Default for Background<E> {
    #[inline]
    fn default() -> Self {
        Self { running: Mutex::new(None) }
    }
}

impl<E: Send + 'static> Background<E> {
    #[inline]
    pub fn cancel(&self) {
        self.replace(None);
    }

    fn replace(&self, next: Option<Running<E>>) {
        let old = mem::replace(&mut *self.running.lock_unpoisoned(), next);
        if let Some(running) = old {
            running.task.abort();
        }
    }

    /// Runs `work`, given where to send its events, replacing any work already running.
    /// Must be called inside a tokio runtime.
    #[inline]
    pub fn start<Callback, Fut>(&self, work: Callback)
    where
        Fut: Future<Output = ()> + Send + 'static,
        Callback: FnOnce(UnboundedSender<E>) -> Fut,
    {
        let (tx, events) = unbounded_channel();
        let task = tokio::spawn(work(tx));
        self.replace(Some(Running { events, task }));
    }

    #[inline]
    pub fn try_next(&self) -> Option<E> {
        self.running.lock_unpoisoned().as_mut()?.events.try_recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use super::*;
    use tokio::time;

    async fn next(background: &Background<u32>) -> u32 {
        time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(event) = background.try_next() {
                    return event;
                }
                time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("an event should arrive")
    }

    #[tokio::test]
    async fn events_arrive_in_order() {
        let background = Background::default();
        background.start(|tx| async move {
            let _sent_one = tx.send(1);
            let _sent_two = tx.send(2);
        });
        assert_eq!((next(&background).await, next(&background).await), (1, 2));
        assert_eq!(background.try_next(), None);
    }

    #[tokio::test]
    async fn starting_again_replaces_the_work_and_drops_what_it_had_said() {
        let background = Background::default();
        background.start(|tx| async move {
            let _sent_one = tx.send(1);
        });
        time::sleep(Duration::from_millis(20)).await;
        background.start(|tx| async move {
            let _sent_two = tx.send(2);
        });
        assert_eq!(next(&background).await, 2);
        assert_eq!(background.try_next(), None);
    }

    #[tokio::test]
    async fn cancelling_stops_the_work_and_drops_its_events() {
        let background = Background::default();
        background.start(|tx| async move {
            let _sent_one = tx.send(1i32);
            time::sleep(Duration::from_millis(30)).await;
            let _sent_two = tx.send(2i32);
        });
        time::sleep(Duration::from_millis(10)).await;
        background.cancel();
        time::sleep(Duration::from_millis(60)).await;
        assert_eq!(background.try_next(), None);
    }
}
