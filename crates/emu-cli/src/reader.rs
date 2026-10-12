//! Whether whoever reads an output of the process is still there.
//!
//! A write to a pipe whose reader went away fails, and that is how a run that prints learns it
//! (`inkemu run | head`). A run that prints nothing writes nothing, and would never learn it:
//! the question has to be asked.

/// Whether the reader of `output` went away: the read end of a pipe was closed, or a terminal was
/// hung up. Asked without waiting (a `poll` with a timeout of zero) and without writing anything.
///
/// The kernel reports the condition as `POLLERR` (Linux, for the write end of a pipe without a
/// reader) or `POLLHUP` (a terminal that was hung up; other systems for the pipe): either is a
/// yes. Anything else is a no, a descriptor `poll` cannot watch (`POLLNVAL`) and a failure of
/// `poll` itself included: a run is never ended on a doubt. A regular file and `/dev/null` have
/// no reader to lose.
#[cfg(unix)]
#[must_use]
pub fn is_gone(output: std::os::fd::BorrowedFd<'_>) -> bool {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    // No event is asked for: `POLLERR` and `POLLHUP` are reported whatever is.
    let mut watched = [PollFd::new(output, PollFlags::empty())];
    if poll(&mut watched, PollTimeout::ZERO).is_err() {
        return false;
    }
    let [watched] = watched;
    watched
        .revents()
        .is_some_and(|events| events.intersects(PollFlags::POLLERR | PollFlags::POLLHUP))
}

#[cfg(all(test, unix))]
mod tests {
    use super::is_gone;
    use std::os::fd::AsFd;

    #[test]
    fn a_pipe_whose_reader_is_there_has_not_lost_it() {
        let (reader, writer) = std::io::pipe().unwrap();
        assert!(!is_gone(writer.as_fd()));
        drop(reader);
    }

    #[test]
    fn a_pipe_whose_reader_was_closed_has_lost_it() {
        let (reader, writer) = std::io::pipe().unwrap();
        drop(reader);
        assert!(is_gone(writer.as_fd()));
    }

    #[test]
    fn a_pipe_keeps_its_reader_while_one_copy_of_the_read_end_is_open() {
        let (reader, writer) = std::io::pipe().unwrap();
        let copy = reader.try_clone().unwrap();
        drop(reader);
        assert!(!is_gone(writer.as_fd()));
        drop(copy);
        assert!(is_gone(writer.as_fd()));
    }

    #[test]
    fn a_regular_file_and_the_null_device_have_no_reader_to_lose() {
        let file = tempfile::tempfile().unwrap();
        assert!(!is_gone(file.as_fd()));
        let null = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/null")
            .unwrap();
        assert!(!is_gone(null.as_fd()));
    }
}
