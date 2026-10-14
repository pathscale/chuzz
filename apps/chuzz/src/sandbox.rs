//! Seatbelt is installed once, before workers and before page bytes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
mod probe;

pub struct Directories {
    pub cache: PathBuf,
    pub profile: PathBuf,
    pub home: PathBuf,
    pub reads: Vec<PathBuf>,
    pub shell: PathBuf,
}

static DIRECTORIES: OnceLock<Result<Arc<Directories>, String>> = OnceLock::new();

pub fn startup() {
    if let Err(reason) = ensure() {
        eprintln!("chuzz: sandbox-startup: {reason}");
        std::process::exit(1);
    }
    if std::env::args().any(|argument| argument == "--sandbox-probe") {
        #[cfg(target_os = "macos")]
        {
            let checks = probe::checks();
            // UNAVAILABLE is a named, reported state, not a pass: the OS does not
            // offer that protection to this process.
            let unavailable = |check: &probe::Check| {
                check.name == "heap-reports-disabled"
                    && !check.passed
                    && crate::platform_hardening::heap_policy_unavailable()
            };
            for check in checks.iter() {
                let verdict = if check.passed {
                    "PASS"
                } else if unavailable(check) {
                    "UNAVAILABLE"
                } else {
                    "FAIL"
                };
                println!("sandbox2: {} {}", check.name, verdict);
            }
            std::process::exit(i32::from(
                checks
                    .iter()
                    .any(|check| !check.passed && !unavailable(check)),
            ));
        }
        #[cfg(not(target_os = "macos"))]
        {
            eprintln!("chuzz: sandbox-probe requires macOS Seatbelt");
            std::process::exit(2);
        }
    }
}

pub fn ensure() -> Result<Arc<Directories>, String> {
    DIRECTORIES.get_or_init(install).clone()
}

pub fn directories() -> Option<&'static Arc<Directories>> {
    DIRECTORIES.get().and_then(|result| result.as_ref().ok())
}

pub(crate) fn probe_html() -> Option<String> {
    #[cfg(target_os = "macos")]
    return Some(probe::html());
    #[cfg(not(target_os = "macos"))]
    None
}

fn own_directory(path: &Path, home: &Path) -> Result<PathBuf, String> {
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    if !path.is_dir()
        || home.starts_with(&path)
        || [
            "/",
            "/Users",
            "/private",
            "/private/tmp",
            "/private/var",
            "/private/etc",
            "/private/var/db",
            "/bin",
            "/sbin",
            "/System",
            "/Library",
            "/usr",
            "/opt",
        ]
        .iter()
        .any(|root| path == Path::new(root))
    {
        return Err(
            "sandbox-directory-too-broad: select a browser or content directory".to_owned(),
        );
    }
    Ok(path)
}

fn content_root(path: PathBuf, home: &Path, reads: &mut Vec<PathBuf>) -> Result<(), String> {
    let path = path.canonicalize().map_err(|error| error.to_string())?;
    let root = if path.is_dir() {
        path
    } else {
        path.parent()
            .ok_or("content path has no directory")?
            .to_path_buf()
    };
    let root = own_directory(&root, home)?;
    if !reads.contains(&root) {
        reads.push(root);
    }
    Ok(())
}

fn install() -> Result<Arc<Directories>, String> {
    crate::platform_hardening::ensure()?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("sandbox-home-missing")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let cache = std::env::var_os("CHUZZ_CACHE_DIR")
        .or_else(|| std::env::var_os("CHUZZ_PROFILE_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("Library/Caches/ai.chuzz.browser"));
    std::fs::create_dir_all(&cache).map_err(|error| format!("sandbox-cache: {error}"))?;
    let cache = own_directory(&cache, &home)?;
    let profile = std::env::var_os("CHUZZ_PROFILE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| cache.join("profile"));
    let old_profile = home.join("Library/Application Support/ai.chuzz.browser");
    if std::env::var_os("CHUZZ_PROFILE_DIR").is_none() && !profile.exists() && old_profile.is_dir()
    {
        let old_profile = own_directory(&old_profile, &home)?;
        let staging = cache.join("profile-import");
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(|error| error.to_string())?;
        }
        migrate_profile(&old_profile, &staging)?;
        std::fs::rename(&staging, &profile).map_err(|error| error.to_string())?;
    }
    std::fs::create_dir_all(&profile).map_err(|error| error.to_string())?;
    let profile = own_directory(&profile, &home)?;
    if !profile.starts_with(&cache) {
        return Err("sandbox-profile-outside-cache".to_owned());
    }
    let temp = cache.join("tmp");
    std::fs::create_dir_all(&temp).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    // This entry point runs before the nagoya runtime and application tasks.
    // Native libraries and the GUI inspection descriptor also use this path.
    unsafe {
        std::env::set_var("TMPDIR", &temp);
    }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut reads = vec![own_directory(
        executable.parent().ok_or("executable directory missing")?,
        &home,
    )?];
    if let Some(contents) = executable.parent().and_then(Path::parent)
        && contents.file_name().is_some_and(|name| name == "Contents")
    {
        reads.push(own_directory(contents, &home)?);
    }
    let old_profile = home.join("Library/Application Support/ai.chuzz.browser");
    if old_profile.exists() {
        reads.push(own_directory(&old_profile, &home)?);
    }
    if let Some(paths) = std::env::var_os("CHUZZ_READ_DIRS") {
        for path in std::env::split_paths(&paths) {
            content_root(path, &home, &mut reads)?;
        }
    }
    if let Some(page) = std::env::var_os("QA_INSPECT_PAGE") {
        let path = PathBuf::from(page);
        if path.exists() {
            content_root(path, &home, &mut reads)?;
        }
    }
    for argument in std::env::args()
        .skip(1)
        .filter(|argument| !argument.starts_with('-'))
    {
        let path = PathBuf::from(&argument);
        if path.exists() {
            content_root(path, &home, &mut reads)?;
        } else if let Ok(url) = url::Url::parse(&argument)
            && url.scheme() == "file"
            && let Ok(path) = url.to_file_path()
        {
            content_root(path, &home, &mut reads)?;
        }
    }
    for name in [
        "QA_INSPECT_STORAGE",
        "CHUZZ_PAGE_SERVER_TLS_CERT",
        "CHUZZ_PAGE_SERVER_TLS_KEY",
        "CHUZZ_DEBUG_EVAL",
        "CHUZZ_DEBUG_PRELUDE",
    ] {
        if let Some(path) = std::env::var_os(name) {
            content_root(path.into(), &home, &mut reads)?;
        }
    }
    // Trusted configuration only. No page, script, image or socket response
    // is read here, and the cached configurations survive restriction.
    crate::tls::prepare()?;
    let shell = Path::new("/bin/sh")
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let directories = Arc::new(Directories {
        cache,
        profile,
        home,
        reads,
        shell,
    });
    #[cfg(target_os = "macos")]
    macos::apply(&directories)?;
    Ok(directories)
}

pub(crate) fn defect(name: &str) -> bool {
    #[cfg(feature = "sandbox2-witness")]
    return std::env::var("SANDBOX2_DEFECT").ok().as_deref() == Some(name);
    #[cfg(not(feature = "sandbox2-witness"))]
    {
        let _ = name;
        false
    }
}

fn migrate_profile(source: &Path, destination: &Path) -> Result<(), String> {
    let mut pending = vec![(source.to_path_buf(), destination.to_path_buf())];
    let mut entries = 0usize;
    while let Some((source, destination)) = pending.pop() {
        std::fs::create_dir_all(&destination)
            .map_err(|error| format!("sandbox-profile-migration: {error}"))?;
        for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            entries += 1;
            if entries > 100_000 {
                return Err("sandbox-profile-migration: entry limit exceeded".to_owned());
            }
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            let target = destination.join(entry.file_name());
            if kind.is_dir() {
                pending.push((entry.path(), target));
            } else if kind.is_file() {
                std::fs::copy(entry.path(), target).map_err(|error| error.to_string())?;
            } else {
                return Err("sandbox-profile-migration: symlink or special file refused".to_owned());
            }
        }
    }
    Ok(())
}
