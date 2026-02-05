use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Notify;

pub struct InspectableChannel<T> {
    queue: parking_lot::RwLock<VecDeque<T>>,
    notify: Arc<Notify>,
}

impl<T> InspectableChannel<T> {
    pub fn with_capacity(cap: usize) -> Self {
        InspectableChannel {
            queue: parking_lot::RwLock::new(VecDeque::with_capacity(cap)),
            notify: Arc::new(Notify::new()),
        }
    }

    pub fn send(&self, msg: T) {
        let mut queue = self.queue.write();
        queue.push_back(msg);
        self.notify.notify_one();
    }

    pub async fn recv(&self) -> Option<T> {
        loop {
            {
                let mut queue = self.queue.write();
                if !queue.is_empty() {
                    return queue.pop_front();
                }
                drop(queue);
            }
            self.notify.notified().await;
        }
    }

    pub async fn recv_many(&self, count: usize) -> Vec<T> {
        let mut messages = Vec::new();

        loop {
            {
                let mut queue = self.queue.write();
                let available = std::cmp::min(count - messages.len(), queue.len());
                for _ in 0..available {
                    if let Some(msg) = queue.pop_front() {
                        messages.push(msg);
                    }
                }
            }

            if !messages.is_empty() {
                return messages;
            }

            self.notify.notified().await;
        }
    }

    pub fn len(&self) -> usize {
        self.queue.read().len()
    }

    pub fn inspect(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.queue.read().iter().cloned().collect()
    }
}
