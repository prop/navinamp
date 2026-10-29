//! Broadcast event fan-out.
//!
//! Each [`crate::EngineHandle::subscribe`] call registers a fresh unbounded
//! crossbeam channel; the producer thread sends every event to all live
//! subscribers. Receivers can use non-blocking `try_recv` / `recv_timeout`.
//! Channels whose receiver has been dropped are pruned automatically.

use std::sync::Mutex;

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::Event;

pub(crate) struct Events {
    subscribers: Mutex<Vec<Sender<Event>>>,
}

impl Events {
    pub(crate) fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = unbounded();
        self.subscribers.lock().unwrap().push(tx);
        rx
    }

    /// Send `event` to every subscriber; prune dead ones.
    pub(crate) fn emit(&self, event: Event) {
        self.subscribers
            .lock()
            .unwrap()
            .retain(|tx| tx.send(event.clone()).is_ok());
    }
}
