//! In-process ring buffer of log lines: the same lines that go to stderr,
//! readable by the agent server (`GET /log`) without holding the launch
//! stderr. The `log!` macro is the app's only logging entry point — every
//! former `eprintln!` goes through it, so a state transition visible on
//! stderr is visible to the agent too.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// At most this many lines survive in the buffer.
const CAPACITY: usize = 500;

struct State {
    lines: VecDeque<String>,
    /// Absolute number of the next line: offsets stay stable as old lines
    /// fall out of the ring.
    total: u64,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(State {
            lines: VecDeque::with_capacity(CAPACITY),
            total: 0,
        })
    })
}

/// Push one formatted line into the ring and mirror it to stderr.
pub fn push(line: String) {
    eprintln!("{line}");
    let mut state = state().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.lines.len() == CAPACITY {
        state.lines.pop_front();
    }
    state.lines.push_back(line);
    state.total += 1;
}

/// Read lines with absolute numbers `>= offset`; returns them with the
/// absolute number of the next line to continue from.
pub fn read_from(offset: u64) -> (Vec<String>, u64) {
    let state = state().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let first_abs = state.total - state.lines.len() as u64;
    let lines: Vec<String> = state
        .lines
        .iter()
        .enumerate()
        .skip_while(|(index, _)| first_abs + (*index as u64) < offset)
        .map(|(_, line)| line.clone())
        .collect();
    (lines, state.total)
}

/// Log one line: stderr plus the ring buffer the agent server reads.
/// Formatting matches `eprintln!` exactly.
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::logbuf::push(format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The buffer keeps at most `CAPACITY` lines: older ones fall out, and
    /// offsets continue to advance past the dropped prefix.
    #[test]
    fn ring_evicts_oldest_and_offsets_advance() {
        let before = read_from(0);
        let base = before.1;
        for index in 0..(CAPACITY as u64 + 5) {
            push(format!("ring-test-{base}-{index}"));
        }
        let (lines, next) = read_from(0);
        assert_eq!(lines.len(), CAPACITY);
        // The first surviving line is the sixth pushed here.
        assert!(lines[0].contains(&format!("ring-test-{base}-5")));
        assert!(lines.last().unwrap().contains(&format!("ring-test-{base}-{}", CAPACITY as u64 + 4)));
        assert!(next >= base + CAPACITY as u64 + 5);

        // Reading from the middle offset returns the tail from there on.
        let (tail, _) = read_from(next - 3);
        assert_eq!(tail.len(), 3);
        assert!(tail[0].contains(&format!("ring-test-{base}-{}", CAPACITY as u64 + 2)));
    }

    /// Reading from a future offset returns nothing and a sane continuation.
    #[test]
    fn read_from_future_offset_is_empty() {
        let (_, next) = read_from(0);
        let (lines, next2) = read_from(next + 1_000_000);
        assert!(lines.is_empty());
        assert!(next2 >= next);
    }
}
