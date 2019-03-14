use std::io;
use std::path::PathBuf;

use crate::recorder::Recorder;

pub fn gc() -> io::Result<()> {
    dotenv::dotenv().ok();

    let work_dir_path = dotenv::var("WORK_DIR").unwrap_or_else(|_| "var".to_owned());
    let work_dir_path = PathBuf::from(work_dir_path);

    let recorder = Recorder::new(work_dir_path);

    recorder.prune_job_dirs()
}
