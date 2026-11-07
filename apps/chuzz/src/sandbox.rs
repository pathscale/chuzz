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
    pub metal_caches: Vec<PathBuf>,
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
        own_directory(&path, home)?
    } else if path.is_file() {
        path
    } else {
        return Err("sandbox-content-not-regular".to_owned());
    };
    if !reads.contains(&root) {
        reads.push(root);
    }
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
const METAL_CACHE_NAMES: [&str; 5] = [
    "com.pathscale.chuzz",
    "chuzz-gui",
    "com.apple.metal",
    "com.apple.metalfe",
    "com.apple.gpuarchiver",
];

#[cfg(any(target_os = "macos", test))]
fn collect_metal_caches(darwin_cache: &Path, home: &Path) -> Vec<PathBuf> {
    let mut metal_caches = Vec::new();
    for name in METAL_CACHE_NAMES {
        let expected = darwin_cache.join(name);
        match std::fs::create_dir(&expected) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => continue,
        }

        let Ok(metadata) = std::fs::symlink_metadata(&expected) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let Ok(canonical) = expected.canonicalize() else {
            continue;
        };
        if canonical != expected || canonical.parent() != Some(darwin_cache) {
            continue;
        }
        let Ok(owned) = own_directory(&expected, home) else {
            continue;
        };
        if owned == expected && !metal_caches.contains(&owned) {
            metal_caches.push(owned);
        }
    }
    metal_caches
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
        if let Some(bundle) = contents.parent()
            && bundle.extension().is_some_and(|ext| ext == "app")
        {
            reads.push(own_directory(bundle, &home)?);
        }
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
    // Inline storage seeds require no filesystem access. Existing seed files
    // still need admission before Seatbelt is applied; missing paths are left
    // to the seed reader so it can report the storage configuration error.
    if let Some(seed) = std::env::var_os("QA_INSPECT_STORAGE") {
        let inline = seed
            .to_str()
            .is_some_and(|text| text.trim_start().starts_with('{'));
        let path = PathBuf::from(seed);
        if !inline && path.exists() {
            content_root(path, &home, &mut reads)?;
        }
    }
    for name in [
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
    let mut metal_caches = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; 4096];
        let len = unsafe {
            libc::confstr(
                libc::_CS_DARWIN_USER_CACHE_DIR,
                buf.as_mut_ptr().cast::<libc::c_char>(),
                buf.len(),
            )
        };
        if len > 0 && len <= buf.len() {
            if let Ok(cstr) = std::ffi::CStr::from_bytes_until_nul(&buf)
                && let Ok(text) = cstr.to_str()
            {
                let darwin_cache = PathBuf::from(text);
                if let Ok(darwin_cache) = darwin_cache.canonicalize() {
                    metal_caches = collect_metal_caches(&darwin_cache, &home);
                }
            }
        }
    }
    let directories = Arc::new(Directories {
        cache,
        profile,
        home,
        reads,
        shell,
        metal_caches,
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

#[cfg(all(test, unix))]
mod metal_cache_tests {
    use super::{METAL_CACHE_NAMES, collect_metal_caches};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    const FIXTURE_ROOT_ENV: &str = "CHUZZ_TEST_SCRATCH_ROOT";
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        path: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::var_os(FIXTURE_ROOT_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    panic!("set {FIXTURE_ROOT_ENV} to an existing owned writable scratch directory")
                });
            Self::new_in(&root, NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed))
        }

        fn new_in(root: &Path, first_id: u64) -> Self {
            let root = root
                .canonicalize()
                .expect("canonicalize owned scratch root");
            assert!(root.is_dir(), "scratch root must be a directory");

            for id in first_id..first_id.saturating_add(128) {
                let path = root.join(format!("chuzz-sandbox-fixture-{}-{id}", std::process::id()));
                match std::fs::create_dir(&path) {
                    Ok(()) => {
                        let canonical = path.canonicalize().expect("canonicalize new fixture");
                        assert_eq!(canonical, path, "fixture must remain a direct child");
                        assert_eq!(canonical.parent(), Some(root.as_path()));
                        return Self {
                            root,
                            path: canonical,
                        };
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create exclusive fixture directory: {error}"),
                }
            }
            panic!("could not allocate a unique fixture directory under scratch root");
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.path.parent() != Some(self.root.as_path()) {
                return;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&self.path) else {
                return;
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return;
            }
            if self.path.canonicalize().ok().as_deref() != Some(self.path.as_path()) {
                return;
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn fixture_retries_existing_directory_and_symlink_collisions() {
        use std::os::unix::fs::symlink;

        let collision_root_owner = Fixture::new();
        let root = collision_root_owner.path();
        let first_id = 0;
        let directory_collision = root.join(format!(
            "chuzz-sandbox-fixture-{}-{first_id}",
            std::process::id()
        ));
        let symlink_collision = root.join(format!(
            "chuzz-sandbox-fixture-{}-{}",
            std::process::id(),
            first_id + 1
        ));
        let witness = root.join(format!(
            "chuzz-sandbox-collision-witness-{}-{first_id}",
            std::process::id()
        ));
        std::fs::create_dir(&directory_collision).expect("create preexisting directory collision");
        let sentinel = directory_collision.join("sentinel");
        std::fs::write(&sentinel, b"preserve directory collision")
            .expect("write collision sentinel");
        std::fs::create_dir(&witness).expect("create symlink collision target");
        symlink(&witness, &symlink_collision).expect("create preexisting symlink collision");

        let fixture = Fixture::new_in(root, first_id);
        assert_ne!(fixture.path(), directory_collision);
        assert_ne!(fixture.path(), symlink_collision);
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"preserve directory collision"
        );
        assert!(
            std::fs::symlink_metadata(&directory_collision)
                .unwrap()
                .is_dir()
        );
        assert!(
            std::fs::symlink_metadata(&symlink_collision)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_link(&symlink_collision).unwrap(), witness);

        drop(fixture);
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"preserve directory collision"
        );
        assert!(
            std::fs::symlink_metadata(&symlink_collision)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_link(&symlink_collision).unwrap(), witness);
        std::fs::remove_file(&symlink_collision).expect("remove test symlink collision");
        std::fs::remove_dir_all(&directory_collision).expect("remove test directory collision");
        std::fs::remove_dir_all(&witness).expect("remove test symlink target");
        drop(collision_root_owner);
    }

    #[test]
    fn admits_only_exact_direct_metal_cache_directories() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let cache = fixture.path().join("cache");
        let home = fixture.path().join("home");
        let sibling = fixture.path().join("unrelated-sibling");
        std::fs::create_dir(&cache).expect("create synthetic cache root");
        std::fs::create_dir(&home).expect("create synthetic home");
        let cache = cache.canonicalize().expect("canonicalize fixture cache");
        let home = home.canonicalize().expect("canonicalize fixture home");
        std::fs::create_dir(&sibling).expect("create unrelated sibling");
        let sentinel = sibling.join("sentinel");
        std::fs::write(&sentinel, b"untouched").expect("write unrelated sentinel");

        let external_link = cache.join("com.apple.metal");
        symlink(&sibling, &external_link).expect("create escaping symlink");
        let positive = cache.join("com.pathscale.chuzz");
        std::fs::create_dir(&positive).expect("create positive-control child");
        let in_root_target = positive.clone();
        let in_root_link = cache.join("com.apple.gpuarchiver");
        symlink(&in_root_target, &in_root_link).expect("create in-root symlink");
        let dangling_link = cache.join("com.apple.metalfe");
        symlink(cache.join("missing-target"), &dangling_link).expect("create dangling symlink");
        let admitted = collect_metal_caches(&cache, &home);
        let expected: Vec<PathBuf> = ["com.pathscale.chuzz", "chuzz-gui"]
            .into_iter()
            .map(|name| cache.join(name))
            .collect();

        assert!(admitted.contains(&positive.canonicalize().unwrap()));
        assert!(!admitted.contains(&cache.join("com.apple.gpuarchiver")));
        assert!(!admitted.contains(&external_link));
        assert!(!admitted.contains(&sibling));
        assert!(!admitted.contains(&dangling_link));
        assert!(admitted.contains(&cache.join("chuzz-gui")));
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"untouched");
        assert!(
            std::fs::symlink_metadata(&external_link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_link(&external_link).unwrap(), sibling);
        assert!(
            std::fs::symlink_metadata(&in_root_link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_link(&in_root_link).unwrap(), in_root_target);
        assert!(
            std::fs::symlink_metadata(&dangling_link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_link(&dangling_link).unwrap(),
            cache.join("missing-target")
        );
        for path in &admitted {
            assert!(
                METAL_CACHE_NAMES
                    .iter()
                    .any(|name| path == &cache.join(name))
            );
            assert_eq!(path.parent(), Some(cache.as_path()));
        }
        assert_eq!(admitted, expected);
        assert!(in_root_target.is_dir());
    }

    #[test]
    fn regular_file_at_allowlisted_child_does_not_suppress_valid_siblings() {
        let fixture = Fixture::new();
        let cache = fixture.path().join("cache");
        let home = fixture.path().join("home");
        std::fs::create_dir(&cache).expect("create synthetic cache root");
        std::fs::create_dir(&home).expect("create synthetic home");
        let cache = cache.canonicalize().expect("canonicalize fixture cache");
        let home = home.canonicalize().expect("canonicalize fixture home");
        let blocked = cache.join("com.apple.metal");
        std::fs::write(&blocked, b"leave unchanged").expect("create regular-file child");

        let admitted = collect_metal_caches(&cache, &home);
        assert!(!admitted.contains(&blocked));
        assert!(admitted.contains(&cache.join("com.pathscale.chuzz")));
        assert_eq!(std::fs::read(&blocked).unwrap(), b"leave unchanged");
        for path in &admitted {
            assert!(
                METAL_CACHE_NAMES
                    .iter()
                    .any(|name| path == &cache.join(name))
            );
            assert_eq!(path.parent(), Some(cache.as_path()));
        }
    }
}
