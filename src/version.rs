pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = env!("FRAMELOG_GIT_HASH");
pub const VERSION_INFO: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("FRAMELOG_GIT_HASH"),
    ")"
);
