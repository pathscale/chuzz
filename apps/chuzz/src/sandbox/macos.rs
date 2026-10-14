use std::ffi::{CStr, CString};
use std::path::Path;

use super::Directories;

#[link(name = "sandbox")]
unsafe extern "C" {
    fn sandbox_init(profile: *const libc::c_char, flags: u64, error: *mut *mut libc::c_char) -> libc::c_int;
    fn sandbox_free_error(error: *mut libc::c_char);
}

fn quote(path: &Path) -> Result<String, String> {
    let text = path.to_str().ok_or("sandbox path must be UTF-8")?;
    if text.contains('\0') { return Err("sandbox path contains NUL".to_owned()); }
    Ok(format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\"")))
}

pub(super) fn apply(dirs: &Directories) -> Result<(), String> {
    let mut profile = include_str!("chuzz.sb").replace("@CACHE@", &quote(&dirs.cache)?)
        .replace("@SHELL@", &quote(&dirs.shell)?);
    let reads = dirs.reads.iter().map(|path| quote(path).map(|path| format!("(allow file-read* (subpath {path}))")))
        .collect::<Result<Vec<_>, _>>()?.join("\n");
    profile = profile.replace("@READS@", &reads);
    let process = if super::defect("spawn-denied") {
        "(allow process-exec process-fork)"
    } else { "(deny process-exec process-fork)" };
    profile = profile.replace("@PROCESS@", process);
    if super::defect("passwd-denied") {
        profile.push_str("\n(allow file-read* (literal \"/private/etc/passwd\"))\n");
    }
    if super::defect("home-denied") {
        profile.push_str(&format!("\n(allow file-read* (literal {}))\n", quote(&dirs.home)?));
    }
    if super::defect("write-denied") {
        profile.push_str(&format!("\n(allow file-write* (literal {}))\n", quote(&super::probe::outside_write(dirs))?));
    }
    if super::defect("cache-readable") {
        profile.push_str(&format!("\n(deny file-read-data (literal {}))\n", quote(&dirs.cache.join("sandbox2-readable"))?));
    }
    let profile = CString::new(profile).map_err(|error| error.to_string())?;
    let mut error = std::ptr::null_mut();
    // A zero flags word means literal SBPL, never a named system profile.
    let result = unsafe { sandbox_init(profile.as_ptr(), 0, &mut error) };
    let message = if error.is_null() { None } else {
        let message = unsafe { CStr::from_ptr(error) }.to_string_lossy().into_owned();
        unsafe { sandbox_free_error(error); }
        Some(message)
    };
    if result != 0 { return Err(format!("seatbelt-apply-failed: {}", message.as_deref().unwrap_or("no diagnostic"))); }
    Ok(())
}
