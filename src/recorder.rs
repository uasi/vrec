use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value as Json};

pub struct Recorder {
    work_dir: WorkDir,
}

impl Recorder {
    pub fn new(path: PathBuf) -> Self {
        Recorder {
            work_dir: WorkDir::new(path),
        }
    }

    pub fn spawn_job(&self, command: &str, args: &[&str]) -> io::Result<Job> {
        let job_id = JobId::new();
        let job_dir = self.work_dir.job_dir(&job_id);
        let job = Job::new(job_id, job_dir);
        job.spawn(command, args).map(|_| job)
    }

    pub fn job(&self, job_id: &JobId) -> Option<Job> {
        let job_dir = self.work_dir.job_dir(job_id);
        if job_dir.path().is_dir() {
            Some(Job::new(job_id.clone(), job_dir))
        } else {
            None
        }
    }

    pub fn jobs(&self) -> Vec<Job> {
        self.work_dir
            .job_dirs()
            .map(|(job_id, job_dir)| Job::new(job_id, job_dir))
            .collect()
    }

    pub fn prune_job_dirs(&self) -> io::Result<()> {
        for job in self.jobs() {
            if !job.is_running() && job.file_names().is_empty() {
                println!("removing dir {:?}", &job.job_dir.path);
                fs::remove_dir_all(&job.job_dir.path)?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct JobId(String);

impl JobId {
    pub fn new() -> Self {
        JobId(ulid::Ulid::new().to_string())
    }
}

impl std::fmt::Display for JobId {
    fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.0.fmt(fmt)
    }
}

impl From<String> for JobId {
    fn from(string: String) -> JobId {
        JobId(string)
    }
}

pub struct Job {
    job_id: JobId,
    job_dir: JobDir,
}

impl Job {
    fn new(job_id: JobId, job_dir: JobDir) -> Self {
        Job { job_id, job_dir }
    }

    pub fn id(&self) -> &JobId {
        &self.job_id
    }

    pub fn path(&self) -> &Path {
        self.job_dir.path()
    }

    pub fn invocation(&self) -> Option<Json> {
        let f = self.job_dir.open_file("info/invocation.json").ok()?;
        serde_json::from_reader(BufReader::new(f)).ok()
    }

    pub fn file_names(&self) -> Vec<String> {
        self.job_dir.file_names()
    }

    pub fn is_running(&self) -> bool {
        match self.pid() {
            Ok(pid) => unsafe { libc::kill(pid, 0) == 0 },
            err => {
                dbg!(err).ok();
                false
            }
        }
    }

    fn spawn(&self, command: &str, args: &[&str]) -> io::Result<()> {
        self.job_dir.create_dir("info")?;

        {
            let f = self.job_dir.create_file("info/invocation.json")?;
            let json = json!({ "command": command, "args": &args });
            writeln!(&f, "{}", json)?;
        }

        let stdout = self.job_dir.create_file("info/stdout.txt")?;
        let stderr = self.job_dir.create_file("info/stderr.txt")?;

        let child = Command::new(command)
            .args(args)
            .current_dir(&self.job_dir.path())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()?;

        let pid_file = self.job_dir.create_file("info/pid.txt")?;
        writeln!(&pid_file, "{}", child.id())?;

        Ok(())
    }

    fn pid(&self) -> Result<i32, &'static str> {
        let mut f = self
            .job_dir
            .open_file("info/pid.txt")
            .map_err(|_| "could not open file")?;
        let mut pid = String::new();
        f.read_to_string(&mut pid).map_err(|_| "read failed")?;
        pid.trim_end().parse().map_err(|_| "parse failed")
    }
}

struct WorkDir {
    path: PathBuf,
}

impl WorkDir {
    fn new(path: PathBuf) -> Self {
        WorkDir { path }
    }

    fn job_dir(&self, job_id: &JobId) -> JobDir {
        JobDir::new(self.path.join(&job_id.0))
    }

    fn job_dirs(&self) -> Box<dyn Iterator<Item = (JobId, JobDir)>> {
        let dir_to_job_dir = |path: PathBuf| -> Option<(JobId, JobDir)> {
            let file_name = path.file_name().and_then(|n| n.to_str());
            if let Some(file_name) = file_name {
                Some((JobId(file_name.to_owned()), JobDir::new(path)))
            } else {
                None
            }
        };

        if let Ok(iter) = self.path.read_dir() {
            let iter = iter
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .filter_map(dir_to_job_dir);
            Box::new(iter)
        } else {
            Box::new(std::iter::empty())
        }
    }
}

struct JobDir {
    path: PathBuf,
}

impl JobDir {
    fn new(path: PathBuf) -> Self {
        JobDir { path }
    }

    fn create_dir<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        assert!(path.as_ref().is_relative());
        fs::create_dir_all(self.path.join(path))
    }

    fn create_file<P: AsRef<Path>>(&self, path: P) -> io::Result<fs::File> {
        fs::File::create(self.path.join(path))
    }

    fn open_file<P: AsRef<Path>>(&self, path: P) -> io::Result<fs::File> {
        fs::File::open(self.path.join(path))
    }

    fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Returns non-hidden file names.
    fn file_names(&self) -> Vec<String> {
        if let Ok(iter) = self.path.read_dir() {
            iter.flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .filter_map(|path| {
                    path.file_name()
                        .and_then(|os_str| os_str.to_str())
                        .and_then(|name| {
                            if !name.starts_with('.') {
                                Some(name.to_owned())
                            } else {
                                None
                            }
                        })
                })
                .collect()
        } else {
            vec![]
        }
    }
}

/// Starts a thread that cleans up exitted child processes.
pub fn start_child_reaper() {
    let signals = signal_hook::iterator::Signals::new(&[signal_hook::SIGCHLD])
        .expect("SIGCHLD handler must be registered");

    std::thread::spawn(move || {
        for _ in signals.forever() {
            loop {
                let pid = unsafe { libc::waitpid(-1, std::ptr::null_mut(), libc::WNOHANG) };
                if pid <= 0 {
                    break;
                }
            }
        }
    });
}
