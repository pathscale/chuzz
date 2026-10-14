use blitz_traits::net::ResourcePolicy;
use std::sync::Arc;

/// Native application parameters. Page content cannot change process options.
pub(crate) fn from_environment() -> Arc<ResourcePolicy> {
    fn value(name: &str, default: u64) -> u64 {
        match std::env::var(name) {
            Ok(text) => text.parse::<u64>().unwrap_or_else(|error| {
                panic!("{name} must be an unsigned byte or request count: {error}")
            }),
            Err(std::env::VarError::NotPresent) => default,
            Err(error) => panic!("{name}: {error}"),
        }
    }
    let mut policy = ResourcePolicy::default();
    policy.max_requests = value("CHUZZ_RESOURCE_MAX_REQUESTS", policy.max_requests);
    policy.warn_requests = value("CHUZZ_RESOURCE_WARN_REQUESTS", policy.warn_requests);
    policy.max_bytes = value("CHUZZ_RESOURCE_MAX_BYTES", policy.max_bytes);
    policy.warn_bytes = value("CHUZZ_RESOURCE_WARN_BYTES", policy.warn_bytes);
    assert!(
        policy.max_requests > 0 && policy.max_bytes > 0,
        "resource caps must be positive"
    );
    assert!(
        policy.warn_requests <= policy.max_requests && policy.warn_bytes <= policy.max_bytes,
        "resource warning thresholds must not exceed caps"
    );
    Arc::new(policy)
}
