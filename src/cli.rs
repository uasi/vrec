use std::io;
use std::path::PathBuf;

use crate::recorder::Recorder;

pub fn gc() -> io::Result<()> {
    dotenv::dotenv().ok();

    let var_dir_path = dotenv::var("VAR_DIR").unwrap_or_else(|_| "var".to_owned());
    let recorder_dir_path = PathBuf::from(var_dir_path).join("jobs");

    let recorder = Recorder::new(recorder_dir_path);

    recorder.prune_job_dirs()
}
