//! Priority queue scheduler, rate draining, and transfer triggering.

use std::collections::VecDeque;
use zenith_session::SessionId;

/// Priority level for queued players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum QueuePriority {
    Standard = 0,
    Vip = 1,
    Staff = 2,
}

/// An entry waiting inside the transit queue.
#[derive(Debug, Clone)]
pub struct QueueEntry {
    pub session_id: SessionId,
    pub priority: QueuePriority,
}

/// The FIFO transit queue manager.
#[derive(Debug, Default)]
pub struct TransitQueue {
    queue: VecDeque<QueueEntry>,
}

impl TransitQueue {
    #[must_use]
    pub fn new() -> Self {
        Self {
            queue: VecDeque::new(),
        }
    }

    /// Enqueues a session based on priority.
    pub fn enqueue(&mut self, session_id: SessionId, priority: QueuePriority) {
        let entry = QueueEntry {
            session_id,
            priority,
        };
        // Find insert position to keep VIP/Staff ahead while preserving FIFO within priority tier
        let idx = self.queue.iter().position(|e| e.priority < priority);
        match idx {
            Some(pos) => self.queue.insert(pos, entry),
            None => self.queue.push_back(entry),
        }
    }

    /// Pops the next player eligible for transfer.
    pub fn dequeue(&mut self) -> Option<QueueEntry> {
        self.queue.pop_front()
    }

    /// Returns the 1-based position of a session in queue, if present.
    #[must_use]
    pub fn position_of(&self, session_id: SessionId) -> Option<usize> {
        self.queue
            .iter()
            .position(|e| e.session_id == session_id)
            .map(|idx| idx + 1)
    }

    /// Total count of players currently queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Checks if the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_ordering() {
        let mut queue = TransitQueue::new();
        let s1 = SessionId(1);
        let s2 = SessionId(2);
        let s3 = SessionId(3);

        queue.enqueue(s1, QueuePriority::Standard);
        queue.enqueue(s2, QueuePriority::Vip);
        queue.enqueue(s3, QueuePriority::Standard);

        // s2 (Vip) should be dequeued first, then s1, then s3
        assert_eq!(queue.dequeue().map(|e| e.session_id), Some(s2));
        assert_eq!(queue.dequeue().map(|e| e.session_id), Some(s1));
        assert_eq!(queue.dequeue().map(|e| e.session_id), Some(s3));
    }
}
