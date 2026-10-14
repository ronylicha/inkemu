//! QEMU in the pinned Docker image (decision H6): the container, the named pipe its trace comes
//! through, and their removal.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// The image QEMU runs in: the one the reference firmware is built in
/// (`tools/conformance/versions.env`, `ESP_IDF_VERSION`).
pub const DEFAULT_IMAGE: &str = "espressif/idf:v5.5.2";
/// QEMU in that image: the build ESP-IDF v5.5.2 pins, `esp_develop_9.2.2_20250817`
/// (`docs/research/sp1b-test-oracle.md` §1a).
pub const DEFAULT_QEMU: &str =
    "/opt/esp/tools/qemu-riscv32/esp_develop_9.2.2_20250817/qemu/bin/qemu-system-riscv32";
/// Where the flash image and the pipe appear inside the container.
const FLASH_IN_CONTAINER: &str = "/flash/flash.bin";
const TRACE_DIR_IN_CONTAINER: &str = "/trace";
/// How often the wait for QEMU to open its end of the pipe looks at the container.
const OPEN_POLL: Duration = Duration::from_millis(200);

/// QEMU's arguments after the flash image: the machine, its virtual clock (`-icount 3`, 8 ns per
/// instruction), one instruction per translation block so that every instruction is logged, the
/// state before each (`cpu`), the traps between (`int`), and no chaining, which would skip the
/// log of chained blocks (oracle §1c). The serial ports go nowhere: the trace is the only output.
///
/// `sleep=off`: while the CPU waits for an interrupt, QEMU's virtual clock jumps to the next
/// timer deadline, as inkemu's device time does. With the default (`sleep=on`) it follows the
/// host's clock while the CPU is idle, so the counters a tick handler reads after a `wfi` depend
/// on the host, and so does the number of instructions of a run (measured: two runs of the
/// reference image, 21 instructions apart over its first life).
fn qemu_arguments(flash: &str, trace: &str) -> Vec<String> {
    [
        "-display",
        "none",
        "-monitor",
        "none",
        "-serial",
        "null",
        "-machine",
        "esp32c3",
        "-icount",
        "shift=3,sleep=off",
        "-drive",
        &format!("file={flash},if=mtd,format=raw,snapshot=on"),
        "-accel",
        "tcg,one-insn-per-tb=on",
        "-d",
        "cpu,nochain,int",
        "-D",
        trace,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum QemuError {
    #[error("cannot create the directory of the pipe {path}: {source}")]
    Directory {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot create the pipe {path}: {source}")]
    Pipe { path: PathBuf, source: nix::Error },
    #[error("cannot start docker: {0}; is Docker installed and running?")]
    Docker(std::io::Error),
    #[error(
        "the container ended before QEMU opened its trace ({status}); is the image {image} present (docker pull {image})?"
    )]
    EndedEarly { status: String, image: String },
    #[error("cannot open the pipe {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// A running QEMU container and the pipe its trace comes through.
#[derive(Debug)]
pub struct Qemu {
    name: String,
    child: Child,
    directory: PathBuf,
}

impl Qemu {
    /// Starts QEMU on `flash` (mounted read-only) in `image`, and returns it with the read end of
    /// the pipe its trace is written to, once QEMU has opened the other end.
    ///
    /// # Errors
    /// [`QemuError`] when the pipe cannot be made, Docker cannot be started, or the container
    /// ends before QEMU opens the pipe.
    pub fn start(flash: &Path, image: &str, qemu: &str) -> Result<(Qemu, File), QemuError> {
        let unique = format!(
            "inkemu-lockstep-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        );
        let directory = std::env::temp_dir().join(&unique);
        std::fs::create_dir(&directory).map_err(|source| QemuError::Directory {
            path: directory.clone(),
            source,
        })?;
        let pipe = directory.join("trace");
        nix::unistd::mkfifo(
            &pipe,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .map_err(|source| QemuError::Pipe {
            path: pipe.clone(),
            source,
        })?;
        let mut command = Command::new("docker");
        command
            .args(["run", "--rm", "--name", &unique, "--network", "none"])
            .arg("-v")
            .arg(format!("{}:{FLASH_IN_CONTAINER}:ro", flash.display()))
            .arg("-v")
            .arg(format!("{}:{TRACE_DIR_IN_CONTAINER}", directory.display()))
            .args(["--entrypoint", qemu, image])
            .args(qemu_arguments(
                FLASH_IN_CONTAINER,
                &format!("{TRACE_DIR_IN_CONTAINER}/trace"),
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        let child = command.spawn().map_err(QemuError::Docker)?;
        let mut qemu = Qemu {
            name: unique,
            child,
            directory,
        };
        let file = qemu.open_pipe(&pipe, image)?;
        Ok((qemu, file))
    }

    /// Opens the read end of the pipe, which blocks until QEMU opens the other; gives up when the
    /// container ends first.
    fn open_pipe(&mut self, pipe: &Path, image: &str) -> Result<File, QemuError> {
        let (sender, receiver) = mpsc::channel();
        let path = pipe.to_path_buf();
        std::thread::spawn(move || {
            let _ = sender.send(File::open(&path));
        });
        loop {
            match receiver.recv_timeout(OPEN_POLL) {
                Ok(opened) => {
                    return opened.map_err(|source| QemuError::Open {
                        path: pipe.to_path_buf(),
                        source,
                    });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(QemuError::Open {
                        path: pipe.to_path_buf(),
                        source: std::io::Error::other("the thread that opens the pipe ended"),
                    });
                }
            }
            if let Ok(Some(status)) = self.child.try_wait() {
                // Unblocks the thread waiting in `open`: a writer appears and leaves at once.
                let _ = std::fs::OpenOptions::new().write(true).open(pipe);
                return Err(QemuError::EndedEarly {
                    status: status.to_string(),
                    image: image.to_owned(),
                });
            }
        }
    }

    /// Stops the container, waits for `docker run` to end and removes the pipe.
    pub fn stop(mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qemu_traces_every_instruction_with_the_virtual_clock_of_the_oracle() {
        let arguments = qemu_arguments("/flash/flash.bin", "/trace/trace").join(" ");
        assert!(arguments.contains("-icount shift=3,sleep=off"));
        assert!(arguments.contains("-accel tcg,one-insn-per-tb=on"));
        assert!(arguments.contains("-d cpu,nochain,int"));
        assert!(arguments.contains("file=/flash/flash.bin,if=mtd,format=raw,snapshot=on"));
        assert!(arguments.ends_with("-D /trace/trace"));
    }
}
