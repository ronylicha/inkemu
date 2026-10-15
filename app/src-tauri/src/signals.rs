//! What `SIGINT` and `SIGTERM` do (2c-R17), apart from Tauri so that it is tested.
//!
//! `ctrlc` calls its handler on one thread, one signal after the other: a handler that waited
//! for the power-off would hold the second signal back. So the first signal starts the power-off
//! on a thread of its own and returns at once; that thread waits for every power-off under way,
//! then exits, whatever the power-off did and whatever a closing of the window is doing. Any later
//! signal exits at once: what a power-off under way had not written yet is then lost.
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The exit status after a signal, as a shell reports a process ended by `SIGINT`.
pub const SIGNALLED: i32 = 130;

type Action = Arc<dyn Fn() + Send + Sync>;
type Exit = Arc<dyn Fn(i32) + Send + Sync>;

/// The handling of the signals of one application.
pub struct Signals {
    received: AtomicBool,
    power_off: Action,
    exit_after: Exit,
    exit_now: Exit,
}

impl Signals {
    /// `power_off` powers the device off and waits for every power-off under way; `exit_after`
    /// ends the application once it has; `exit_now` ends the process at once.
    pub fn new(power_off: Action, exit_after: Exit, exit_now: Exit) -> Signals {
        Signals {
            received: AtomicBool::new(false),
            power_off,
            exit_after,
            exit_now,
        }
    }

    /// One signal: the first starts the power-off on its own thread and returns at once; any
    /// later one exits at once.
    pub fn receive(&self) {
        if self.received.swap(true, Ordering::SeqCst) {
            (self.exit_now)(SIGNALLED);
            return;
        }
        let power_off = Arc::clone(&self.power_off);
        let exit_after = Arc::clone(&self.exit_after);
        let run = move || {
            // The application exits whatever the power-off did, a panic included: a signal that
            // does not end it would be lost.
            let _ = catch_unwind(AssertUnwindSafe(|| power_off()));
            exit_after(SIGNALLED);
        };
        let spawned = std::thread::Builder::new()
            .name("inkemu-signal".to_owned())
            .spawn(run.clone());
        if spawned.is_err() {
            // No thread to be had: power off here; a second signal then waits for it.
            run();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// A second signal exits at once while the first one's power-off is still under way; the
    /// first one's exit comes once the power-off has ended.
    #[test]
    fn a_second_signal_exits_at_once_while_a_slow_power_off_goes_on() {
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let (release, released) = mpsc::channel::<()>();
        let released = Arc::new(Mutex::new(released));
        let (power_off_seen, exit_after_seen, exit_now_seen) =
            (Arc::clone(&seen), Arc::clone(&seen), Arc::clone(&seen));
        let signals = Signals::new(
            Arc::new(move || {
                power_off_seen
                    .lock()
                    .unwrap()
                    .push("power-off begins".into());
                let _ = released.lock().unwrap().recv();
                power_off_seen.lock().unwrap().push("power-off ends".into());
            }),
            Arc::new(move |code| {
                exit_after_seen.lock().unwrap().push(format!("exit {code}"));
            }),
            Arc::new(move |code| {
                exit_now_seen
                    .lock()
                    .unwrap()
                    .push(format!("exit now {code}"));
            }),
        );
        let first = Instant::now();
        signals.receive();
        assert!(
            first.elapsed() < Duration::from_secs(5),
            "the first signal returns"
        );
        // The power-off is under way and waits: the second signal is handled meanwhile.
        let started = Instant::now();
        while !seen
            .lock()
            .unwrap()
            .contains(&"power-off begins".to_owned())
        {
            assert!(started.elapsed() < Duration::from_secs(60));
            std::thread::yield_now();
        }
        signals.receive();
        assert_eq!(
            *seen.lock().unwrap(),
            ["power-off begins".to_owned(), "exit now 130".to_owned()]
        );
        release.send(()).unwrap();
        let waited = Instant::now();
        while !seen.lock().unwrap().contains(&"exit 130".to_owned()) {
            assert!(waited.elapsed() < Duration::from_secs(60));
            std::thread::yield_now();
        }
        assert_eq!(
            *seen.lock().unwrap(),
            [
                "power-off begins".to_owned(),
                "exit now 130".to_owned(),
                "power-off ends".to_owned(),
                "exit 130".to_owned()
            ]
        );
    }

    /// A power-off that panics still ends in the exit.
    #[test]
    fn a_power_off_that_panics_still_exits() {
        let (exited, exits) = mpsc::channel();
        let signals = Signals::new(
            Arc::new(|| panic!("a power-off broke")),
            Arc::new(move |code| {
                let _ = exited.send(code);
            }),
            Arc::new(|_| unreachable!("one signal only")),
        );
        signals.receive();
        assert_eq!(exits.recv_timeout(Duration::from_secs(60)), Ok(SIGNALLED));
    }
}
