use std::path::Path;

pub struct DiskStat {
    pub available: u64,
    pub total: u64,
    pub used: u64,
}

impl DiskStat {
    pub fn new<P: AsRef<Path>>(path: P) -> Option<DiskStat> {
        let path = std::ffi::CString::new(path.as_ref().to_str()?).ok()?;
        let mut stat = unsafe { std::mem::zeroed() };

        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
            return None;
        }

        let available = u64::from(stat.f_bavail).checked_mul(stat.f_frsize)?;
        let total = u64::from(stat.f_blocks).checked_mul(stat.f_frsize)?;
        let used = total.checked_sub(available)?;

        Some(DiskStat {
            available,
            total,
            used,
        })
    }
}

pub fn humanize_byte_size(size: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

    let size = size as f64;
    let e = ((size.log10() / 3.0).floor() as i32).min((UNITS.len() - 1) as i32);
    format!("{:.3}{}", size / 1000_f64.powi(e), UNITS[e as usize])
}
