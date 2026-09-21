//! LIFO Revertible Effect Stack
//!
//! Tracks side effects and resource guards created by plugins.
//! Guarantees deterministic teardown in reverse registration order (RAII).

/// A reversible effect action pushed onto the LIFO stack
pub struct CleanupGuard {
    name: String,
    action: Option<Box<dyn FnOnce() + Send + Sync + 'static>>,
}

impl CleanupGuard {
    pub fn new(name: impl Into<String>, action: impl FnOnce() + Send + Sync + 'static) -> Self {
        Self {
            name: name.into(),
            action: Some(Box::new(action)),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn execute(mut self) {
        if let Some(action) = self.action.take() {
            action();
        }
    }
}

/// A LIFO (Last-In, First-Out) disposal stack
#[derive(Default)]
pub struct LifoStack {
    guards: Vec<CleanupGuard>,
}

impl LifoStack {
    pub fn new() -> Self {
        Self { guards: Vec::new() }
    }

    /// Push an effect cleanup guard onto the stack
    pub fn push(&mut self, name: impl Into<String>, action: impl FnOnce() + Send + Sync + 'static) {
        self.guards.push(CleanupGuard::new(name, action));
    }

    /// Number of active guards currently registered
    pub fn len(&self) -> usize {
        self.guards.len()
    }

    /// Whether the stack is empty
    pub fn is_empty(&self) -> bool {
        self.guards.is_empty()
    }

    /// Pop and execute all cleanup actions in strict LIFO order
    pub fn unwind(&mut self) {
        while let Some(guard) = self.guards.pop() {
            guard.execute();
        }
    }
}

impl Drop for LifoStack {
    fn drop(&mut self) {
        self.unwind();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_lifo_unwind_order() {
        let execution_order = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut stack = LifoStack::new();

        let order1 = Arc::clone(&execution_order);
        stack.push("effect_1", move || {
            order1.lock().unwrap().push(1);
        });

        let order2 = Arc::clone(&execution_order);
        stack.push("effect_2", move || {
            order2.lock().unwrap().push(2);
        });

        let order3 = Arc::clone(&execution_order);
        stack.push("effect_3", move || {
            order3.lock().unwrap().push(3);
        });

        assert_eq!(stack.len(), 3);

        // Unwind: must execute in reverse order 3 -> 2 -> 1
        stack.unwind();
        assert_eq!(stack.len(), 0);

        let final_order = execution_order.lock().unwrap().clone();
        assert_eq!(final_order, vec![3, 2, 1]);
    }

    #[test]
    fn test_lifo_drop_raii_guarantee() {
        let counter = Arc::new(AtomicUsize::new(0));

        {
            let mut stack = LifoStack::new();
            let c1 = Arc::clone(&counter);
            stack.push("guard_1", move || {
                c1.fetch_add(10, Ordering::SeqCst);
            });
            let c2 = Arc::clone(&counter);
            stack.push("guard_2", move || {
                c2.fetch_add(5, Ordering::SeqCst);
            });
            // stack goes out of scope here without explicit unwind()
        }

        // Both guards must have executed in drop
        assert_eq!(counter.load(Ordering::SeqCst), 15);
    }
}
