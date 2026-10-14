//! QEMU in the pinned Docker image (decision H6): the container, the named pipe its trace comes
//! through, and their removal, whatever way the run ends.

use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The image QEMU runs in: the one the reference firmware is built in
/// (`tools/conformance/versions.env`, `ESP_IDF_VERSION`; a test holds the two equal).
pub const DEFAULT_IMAGE: &str = "espressif/idf:v5.5.2";
/// QEMU in that image: the build ESP-IDF v5.5.2 pins (`tools/conformance/versions.env`,
/// `QEMU_ESP_BUILD`; `docs/research/sp1b-test-oracle.md` §1a).
pub const DEFAULT_QEMU: &str =
    "/opt/esp/tools/qemu-riscv32/esp_develop_9.2.2_20250817/qemu/bin/qemu-system-riscv32";
/// Where the flash image and the pipe appear inside the container.
const FLASH_IN_CONTAINER: &str = "/flash/flash.bin";
const TRACE_DIR_IN_CONTAINER: &str = "/trace";
/// How often the wait for QEMU to open its end of the pipe looks at the container and at Ctrl-C.
const OPEN_POLL: Duration = Duration::from_millis(200);
/// How long QEMU may take to open its trace: it does so while parsing its options, before the
/// machine exists, a second at most on the CI's runners. Past this the container is stuck
/// (a daemon that cannot share the pipe, a QEMU that hangs) and the run fails rather than wait.
const OPEN_TIMEOUT: Duration = Duration::from_secs(120);
/// How long the end of the trace waits for the container to report how QEMU ended.
const EXIT_TIMEOUT: Duration = Duration::from_secs(10);

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
    #[error("{path} cannot be given to Docker as a mount: it must be UTF-8 and hold no comma")]
    Path { path: PathBuf },
    #[error("cannot start docker: {0}; is Docker installed and running?")]
    Docker(std::io::Error),
    #[error(
        "the container ended ({status}) before QEMU opened its trace: see QEMU's message above; \
         check --qemu ({qemu}) and that the image {image} is present (docker pull {image})"
    )]
    EndedEarly {
        status: String,
        qemu: String,
        image: String,
    },
    #[error("QEMU did not open its trace within {} s", OPEN_TIMEOUT.as_secs())]
    Stuck,
    #[error("cannot open the pipe {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("QEMU ended ({status}) while the comparison read its trace: see its message above")]
    Failed { status: String },
}

/// The directory of the pipe, removed when it goes out of scope, on every path out of
/// [`Qemu::start`] and with the [`Qemu`] that owns it.
#[derive(Debug)]
struct PipeDirectory(PathBuf);

impl Drop for PipeDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running QEMU container and the pipe its trace comes through. Dropping it removes the
/// container by its name until Docker says it is gone and `docker run` has exited ([`clean_up`],
/// which a daemon slow to create the container does not outrun), then the pipe, whatever ended
/// the run: an error, a panic, Ctrl-C. A container it cannot show gone is named on stderr.
#[derive(Debug)]
pub struct Qemu {
    name: String,
    child: Child,
    /// Held for its removal when the container is gone: fields drop after [`Qemu::drop`].
    _directory: PipeDirectory,
}

/// Whether the interrupted flag was raised (Ctrl-C).
pub type Interrupted<'a> = &'a AtomicBool;

/// What [`Qemu::start`] ends with.
pub enum Started {
    Running(Qemu, File),
    /// Ctrl-C came while QEMU was starting.
    Interrupted,
}

impl Qemu {
    /// Starts QEMU on `flash` (mounted read-only) in `image`, and returns it with the read end of
    /// the pipe its trace is written to, once QEMU has opened the other end.
    ///
    /// `docker run` is put in a process group of its own: a Ctrl-C at the terminal reaches this
    /// tool only, which then stops the container itself, rather than the container ending its
    /// trace under a comparison that would take the end for the end of QEMU's run.
    ///
    /// # Errors
    /// [`QemuError`] when the pipe cannot be made, a path cannot be mounted, Docker cannot be
    /// started, or the container ends or hangs before QEMU opens the pipe.
    pub fn start(
        flash: &Path,
        image: &str,
        qemu: &str,
        interrupted: Interrupted<'_>,
    ) -> Result<Started, QemuError> {
        let unique = container_name(
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        let path = std::env::temp_dir().join(&unique);
        std::fs::create_dir(&path).map_err(|source| QemuError::Directory {
            path: path.clone(),
            source,
        })?;
        let directory = PipeDirectory(path);
        let pipe = directory.0.join("trace");
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
            .args(["run", "--rm", "--pull=never", "--name", &unique])
            .args(["--network", "none"])
            .arg("--mount")
            .arg(bind_mount(flash, FLASH_IN_CONTAINER, true)?)
            .arg("--mount")
            .arg(bind_mount(&directory.0, TRACE_DIR_IN_CONTAINER, false)?)
            .args(["--entrypoint", qemu, image])
            .args(qemu_arguments(
                FLASH_IN_CONTAINER,
                &format!("{TRACE_DIR_IN_CONTAINER}/trace"),
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .process_group(0);
        let child = command.spawn().map_err(QemuError::Docker)?;
        let mut running = Qemu {
            name: unique,
            child,
            _directory: directory,
        };
        let opened = running.open_pipe(&pipe, image, qemu, interrupted)?;
        Ok(match opened {
            Some(file) => Started::Running(running, file),
            None => Started::Interrupted,
        })
    }

    /// Opens the read end of the pipe, which blocks until QEMU opens the other; gives up when the
    /// container ends first, after [`OPEN_TIMEOUT`], or at Ctrl-C (`None`).
    fn open_pipe(
        &mut self,
        pipe: &Path,
        image: &str,
        qemu: &str,
        interrupted: Interrupted<'_>,
    ) -> Result<Option<File>, QemuError> {
        let (sender, receiver) = mpsc::channel();
        let path = pipe.to_path_buf();
        std::thread::spawn(move || {
            let _ = sender.send(File::open(&path));
        });
        let started = Instant::now();
        loop {
            match receiver.recv_timeout(OPEN_POLL) {
                Ok(opened) => {
                    return opened.map(Some).map_err(|source| QemuError::Open {
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
            let ended = self.child.try_wait().ok().flatten();
            let stuck = started.elapsed() >= OPEN_TIMEOUT;
            let stop = interrupted.load(Ordering::Relaxed);
            if ended.is_some() || stuck || stop {
                // Unblocks the thread waiting in `open`: a writer appears and leaves at once.
                let _ = std::fs::OpenOptions::new().write(true).open(pipe);
            }
            if let Some(status) = ended {
                return Err(QemuError::EndedEarly {
                    status: status.to_string(),
                    qemu: qemu.to_owned(),
                    image: image.to_owned(),
                });
            }
            if stuck {
                return Err(QemuError::Stuck);
            }
            if stop {
                return Ok(None);
            }
        }
    }

    /// How QEMU ended, once its trace has: the trace ends only when QEMU exits, which a firmware
    /// that runs forever never asks for.
    ///
    /// # Errors
    /// [`QemuError::Failed`] when QEMU ended, or had not within [`EXIT_TIMEOUT`].
    pub fn ended(&mut self) -> Result<(), QemuError> {
        let started = Instant::now();
        loop {
            if let Ok(Some(status)) = self.child.try_wait() {
                return check(status);
            }
            if started.elapsed() >= EXIT_TIMEOUT {
                return Err(QemuError::Failed {
                    status: "still running after its trace closed".to_owned(),
                });
            }
            std::thread::sleep(OPEN_POLL);
        }
    }
}

/// QEMU never exits on its own while a firmware runs: any end that is not the success of a
/// guest that asked to stop is a failure of the run.
fn check(status: ExitStatus) -> Result<(), QemuError> {
    if status.success() {
        Ok(())
    } else {
        Err(QemuError::Failed {
            status: status.to_string(),
        })
    }
}

/// How many times [`clean_up`] removes the container and looks again before it gives up, and
/// how long it waits between two tries: 10 s in all.
const REMOVE_TRIES: u32 = 50;
const REMOVE_POLL: Duration = Duration::from_millis(200);

/// What [`clean_up`] needs of Docker, behind a trait so that its logic is tested without one.
trait Docker {
    /// `docker rm --force <name>`; its outcome is read through [`Docker::exists`].
    fn remove(&mut self, name: &str);
    /// Whether the container exists (`docker inspect`): `None` when Docker does not say.
    fn exists(&mut self, name: &str) -> Option<bool>;
}

/// The `docker run` client of the container.
trait Client {
    /// Whether it has exited.
    fn exited(&mut self) -> bool;
    /// Kills it and waits for it.
    fn kill(&mut self);
}

/// How [`clean_up`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cleanup {
    /// The client exited and Docker says the container does not exist.
    Removed,
    /// The client did not exit within the bound: Docker was still starting the container, and
    /// may create it after the client is killed.
    Unconfirmed,
    /// The container still exists, or Docker would not say.
    Left,
}

/// Removes the container `name` without a race with a slow daemon. A container the daemon
/// creates late, after a first removal found nothing, is removed by a later try: the removal is
/// repeated until the client has exited (it does once its container is gone, or never started)
/// and Docker says the container does not exist, [`REMOVE_TRIES`] times at most, `poll` apart.
/// Past that the client is killed and one more removal made.
fn clean_up(
    docker: &mut impl Docker,
    client: &mut impl Client,
    name: &str,
    poll: Duration,
) -> Cleanup {
    for _ in 0..REMOVE_TRIES {
        docker.remove(name);
        if client.exited() && docker.exists(name) == Some(false) {
            return Cleanup::Removed;
        }
        std::thread::sleep(poll);
    }
    let exited = client.exited();
    client.kill();
    docker.remove(name);
    match docker.exists(name) {
        Some(false) if exited => Cleanup::Removed,
        Some(false) => Cleanup::Unconfirmed,
        _ => Cleanup::Left,
    }
}

/// What the user is told when the container could not be shown gone: never nothing.
fn cleanup_warning(cleanup: Cleanup, name: &str) -> Option<String> {
    match cleanup {
        Cleanup::Removed => None,
        Cleanup::Unconfirmed => Some(format!(
            "inkemu-lockstep: Docker had not finished starting the container {name}; if it \
             appears, remove it with `docker rm -f {name}`"
        )),
        Cleanup::Left => Some(format!(
            "inkemu-lockstep: the container {name} could not be removed; remove it with \
             `docker rm -f {name}`"
        )),
    }
}

/// The Docker of the host.
struct HostDocker;

impl Docker for HostDocker {
    fn remove(&mut self, name: &str) {
        let _ = Command::new("docker")
            .args(["rm", "--force", name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    fn exists(&mut self, name: &str) -> Option<bool> {
        let output = Command::new("docker")
            .args([
                "inspect",
                "--type",
                "container",
                "--format",
                "{{.Id}}",
                name,
            ])
            .stdin(Stdio::null())
            .output()
            .ok()?;
        if output.status.success() {
            return Some(true);
        }
        String::from_utf8_lossy(&output.stderr)
            .to_ascii_lowercase()
            .contains("no such")
            .then_some(false)
    }
}

impl Client for Child {
    fn exited(&mut self) -> bool {
        matches!(self.try_wait(), Ok(Some(_)))
    }

    fn kill(&mut self) {
        let _ = Child::kill(self);
        let _ = self.wait();
    }
}

impl Drop for Qemu {
    fn drop(&mut self) {
        let cleanup = clean_up(&mut HostDocker, &mut self.child, &self.name, REMOVE_POLL);
        if let Some(warning) = cleanup_warning(cleanup, &self.name) {
            eprintln!("{warning}");
        }
    }
}

/// The name of a run's container, and of its pipe's directory: the process and the instant make
/// it unique, so that a removal by name never reaches another run's container.
fn container_name(pid: u32, nanos: u128) -> String {
    format!("inkemu-lockstep-{pid}-{nanos}")
}

/// `--mount` for `source` at `target`: a comma would split the option and a path that is not
/// UTF-8 would be mangled, so both are refused.
fn bind_mount(source: &Path, target: &str, readonly: bool) -> Result<String, QemuError> {
    let text = source
        .to_str()
        .filter(|text| !text.contains(','))
        .ok_or_else(|| QemuError::Path {
            path: source.to_path_buf(),
        })?;
    Ok(format!(
        "type=bind,source={text},target={target}{}",
        if readonly { ",readonly" } else { "" }
    ))
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

    #[test]
    fn a_mount_is_refused_a_comma_and_takes_readonly() {
        assert_eq!(
            bind_mount(Path::new("/a/flash.bin"), "/flash/flash.bin", true).unwrap(),
            "type=bind,source=/a/flash.bin,target=/flash/flash.bin,readonly"
        );
        assert!(bind_mount(Path::new("/a,b/flash.bin"), "/f", false).is_err());
    }

    #[test]
    fn the_image_and_qemu_are_those_of_versions_env() {
        let versions = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/conformance/versions.env"),
        )
        .unwrap();
        let value = |key: &str| {
            versions
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{key}=")))
                .unwrap_or_else(|| panic!("{key} is not in versions.env"))
                .to_owned()
        };
        assert_eq!(
            DEFAULT_IMAGE,
            format!("espressif/idf:{}", value("ESP_IDF_VERSION"))
        );
        assert!(
            DEFAULT_QEMU.contains(&format!("/{}/", value("QEMU_ESP_BUILD"))),
            "{DEFAULT_QEMU}"
        );
    }

    /// A Docker whose daemon creates the container on the `appears`-th call (never when `None`),
    /// and removes it only when `removable`.
    struct FakeDocker {
        appears: Option<u32>,
        removable: bool,
        looks: u32,
        exists: bool,
        removed: bool,
        removals: u32,
    }

    impl FakeDocker {
        fn new(appears: Option<u32>, removable: bool) -> FakeDocker {
            FakeDocker {
                appears,
                removable,
                looks: 0,
                exists: false,
                removed: false,
                removals: 0,
            }
        }

        fn tick(&mut self) {
            self.looks += 1;
            if self.appears == Some(self.looks) {
                self.exists = true;
            }
        }
    }

    impl Docker for FakeDocker {
        fn remove(&mut self, name: &str) {
            assert!(name.starts_with("inkemu-lockstep-"), "{name}");
            self.tick();
            self.removals += 1;
            if self.exists && self.removable {
                self.exists = false;
                self.removed = true;
            }
        }

        fn exists(&mut self, _name: &str) -> Option<bool> {
            self.tick();
            Some(self.exists)
        }
    }

    /// A `docker run` client that exits once its container has been removed (`gone`), or never
    /// when `stuck`.
    struct FakeClient<'a> {
        gone: &'a std::cell::Cell<bool>,
        stuck: bool,
        killed: bool,
    }

    impl Client for FakeClient<'_> {
        fn exited(&mut self) -> bool {
            self.killed || (!self.stuck && self.gone.get())
        }

        fn kill(&mut self) {
            self.killed = true;
        }
    }

    /// The fake Docker, telling the fake client when its container is gone.
    struct Linked<'a, 'b> {
        docker: &'a mut FakeDocker,
        done: &'b std::cell::Cell<bool>,
    }

    impl Docker for Linked<'_, '_> {
        fn remove(&mut self, name: &str) {
            self.docker.remove(name);
            self.done.set(self.docker.removed);
        }

        fn exists(&mut self, name: &str) -> Option<bool> {
            self.docker.exists(name)
        }
    }

    /// Runs [`clean_up`] on `docker` and a client linked to it; whether the client was killed.
    fn run(docker: &mut FakeDocker, stuck: bool) -> (Cleanup, bool) {
        let done = std::cell::Cell::new(false);
        let mut client = FakeClient {
            gone: &done,
            stuck,
            killed: false,
        };
        let mut linked = Linked {
            docker,
            done: &done,
        };
        let cleanup = clean_up(
            &mut linked,
            &mut client,
            &container_name(42, 7),
            Duration::ZERO,
        );
        (cleanup, client.killed)
    }

    #[test]
    fn a_container_name_is_unique_to_the_process_and_the_instant() {
        assert_eq!(container_name(42, 7), "inkemu-lockstep-42-7");
        assert_ne!(container_name(42, 7), container_name(42, 8));
        assert_ne!(container_name(42, 7), container_name(43, 7));
    }

    #[test]
    fn a_container_the_daemon_creates_late_is_still_removed() {
        // The first removal finds nothing; the daemon creates the container afterwards, while
        // the client still waits for it, and a later try removes it.
        let mut docker = FakeDocker::new(Some(9), true);
        let (cleanup, killed) = run(&mut docker, false);
        assert_eq!(cleanup, Cleanup::Removed);
        assert!(docker.removed && !docker.exists && !killed);
        assert!(docker.removals > 1, "removed after it appeared");
        assert_eq!(cleanup_warning(cleanup, "inkemu-lockstep-42-7"), None);
    }

    #[test]
    fn a_container_that_will_not_go_or_a_daemon_that_never_starts_it_is_named_on_stderr() {
        let name = container_name(42, 7);
        // The container exists and every removal fails.
        let mut docker = FakeDocker::new(Some(1), false);
        let (cleanup, killed) = run(&mut docker, false);
        assert_eq!(cleanup, Cleanup::Left);
        assert!(killed, "the client is killed past the bound");
        assert_eq!(docker.removals, REMOVE_TRIES + 1);
        let warning = cleanup_warning(cleanup, &name).unwrap();
        assert!(
            warning.contains(&format!("docker rm -f {name}")),
            "{warning}"
        );
        // The daemon never creates the container and the client waits for it forever: the
        // client is killed, and the name is given in case the daemon creates it later.
        let mut docker = FakeDocker::new(None, true);
        let (cleanup, killed) = run(&mut docker, true);
        assert_eq!(cleanup, Cleanup::Unconfirmed);
        assert!(killed);
        let warning = cleanup_warning(cleanup, &name).unwrap();
        assert!(warning.contains(&name), "{warning}");
    }

    #[test]
    fn a_qemu_that_ended_badly_fails_the_run() {
        use std::os::unix::process::ExitStatusExt;
        assert!(check(ExitStatus::from_raw(0)).is_ok());
        assert!(check(ExitStatus::from_raw(1 << 8)).is_err());
    }
}
