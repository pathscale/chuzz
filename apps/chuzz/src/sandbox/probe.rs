use std::fs::{self, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use super::Directories;

pub(super) struct Check {
    pub name: &'static str,
    pub passed: bool,
}

pub(super) fn outside_write(dirs: &Directories) -> PathBuf {
    dirs.home
        .join(format!(".chuzz-sandbox2-write-{}", std::process::id()))
}

fn denied<T>(result: io::Result<T>) -> bool {
    matches!(result, Err(error) if error.kind() == io::ErrorKind::PermissionDenied)
}

pub(super) fn checks() -> Arc<Vec<Check>> {
    static CHECKS: OnceLock<Arc<Vec<Check>>> = OnceLock::new();
    Arc::clone(CHECKS.get_or_init(|| {
        let Some(dirs) = super::directories() else {
            return Arc::new(vec![Check {
                name: "seatbelt-applied",
                passed: false,
            }]);
        };
        let marker = dirs.cache.join("sandbox2-readable");
        let cache = fs::write(&marker, b"chuzz-sandbox2-cache").and_then(|()| fs::read(&marker));
        let outside = outside_write(dirs);
        let write = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&outside);
        let write_denied = denied(write);
        if !write_denied {
            let _ = fs::remove_file(&outside);
        }
        // The shell is a known existing executable, so EPERM proves the spawn
        // restriction rather than a missing pathname. Its command is a builtin.
        let spawn = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .status();
        Arc::new(vec![
            Check {
                name: "passwd-denied",
                passed: denied(fs::read("/etc/passwd")),
            },
            Check {
                name: "home-denied",
                passed: denied(fs::read_dir(&dirs.home)),
            },
            Check {
                name: "write-denied",
                passed: write_denied,
            },
            Check {
                name: "spawn-denied",
                passed: denied(spawn),
            },
            Check {
                name: "cache-readable",
                passed: cache.is_ok_and(|bytes| bytes == b"chuzz-sandbox2-cache"),
            },
            Check {
                name: "core-limit-zero",
                passed: crate::platform_hardening::core_limit_zero(),
            },
            Check {
                name: "heap-reports-disabled",
                passed: crate::platform_hardening::heap_reports_disabled(),
            },
        ])
    }))
}

pub(super) fn html() -> String {
    let checks = checks();
    let mut html = String::from(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Sandbox probe</title></head><body style=\"background:white;color:black;font:18px sans-serif;padding:24px\"><h1>Sandbox probe</h1>",
    );
    for check in checks.iter() {
        let verdict = if check.passed { "PASS" } else { "FAIL" };
        html.push_str(&format!(
            "<p id=\"{}\">{} {verdict}</p>",
            check.name, check.name
        ));
    }
    html.push_str("</body></html>");
    html
}
