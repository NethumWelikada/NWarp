use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Logger {
    access_path: String,
    error_path: String,
    lock: Mutex<()>,
}

impl Logger {
    pub fn new(access_path: &str, error_path: &str) -> Logger {
        ensure_parent_dir(access_path);
        ensure_parent_dir(error_path);
        Logger {
            access_path: access_path.to_string(),
            error_path: error_path.to_string(),
            lock: Mutex::new(()),
        }
    }

    pub fn access(&self, method: &str, path: &str, status: u16, peer: &str) {
        let line = format!(
            "[{}] {} \"{} {}\" {}\n",
            timestamp(),
            peer,
            method,
            path,
            status
        );
        self.write(&self.access_path, &line);
        print!("{}", line);
    }

    pub fn error(&self, msg: &str) {
        let line = format!("[{}] ERROR: {}\n", timestamp(), msg);
        self.write(&self.error_path, &line);
        eprint!("{}", line);
    }

    fn write(&self, path: &str, line: &str) {
        let _guard = self.lock.lock().unwrap();
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

fn ensure_parent_dir(path: &str) {
    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            let _ = fs::create_dir_all(parent);
        }
    }
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
