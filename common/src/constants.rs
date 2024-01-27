//! Shared constants used across Shuttle crates

/// Where executables are moved to in order to persist across deploys, relative to workspace root
pub const EXECUTABLE_DIRNAME: &str = ".shuttle-executables";
/// Where general files will persist across deploys, relative to workspace root. Used by plugins.
pub const STORAGE_DIRNAME: &str = ".shuttle-storage";

// URLs
pub const API_URL_LOCAL: &str = "http://localhost:8001";
pub const API_URL_PRODUCTION: &str = "https://api.shuttle.rs";
#[cfg(debug_assertions)]
pub const API_URL_DEFAULT: &str = API_URL_LOCAL;
#[cfg(not(debug_assertions))]
pub const API_URL_DEFAULT: &str = API_URL_PRODUCTION;

pub const SHUTTLE_STATUS_URL: &str = "https://status.shuttle.rs";
pub const SHUTTLE_LOGIN_URL: &str = "https://console.shuttle.rs/new-project";

pub const SHUTTLE_NEW_ISSUE_URL: &str = "https://github.com/shuttle-hq/shuttle/issues/new/choose";
pub const SHUTTLE_EXAMPLES_REPO: &str = "https://github.com/shuttle-hq/shuttle-examples";
pub const SHUTTLE_EXAMPLES_README: &str =
    "https://github.com/shuttle-hq/shuttle-examples#how-to-clone-run-and-deploy-an-example";

pub const SHUTTLE_INSTALL_DOCS_URL: &str = "https://docs.shuttle.rs/getting-started/installation";
pub const SHUTTLE_CLI_DOCS_URL: &str = "https://docs.shuttle.rs/getting-started/shuttle-commands";
pub const SHUTTLE_IDLE_DOCS_URL: &str = "https://docs.shuttle.rs/getting-started/idle-projects";

// Crate names for checking cargo metadata
pub const NEXT_NAME: &str = "shuttle-next";
pub const RUNTIME_NAME: &str = "shuttle-runtime";

/// Timeframe before a project is considered idle
pub const DEFAULT_IDLE_MINUTES: u32 = 30;

/// Function to set [DEFAULT_IDLE_MINUTES] as a serde default
pub const fn default_idle_minutes() -> u32 {
    DEFAULT_IDLE_MINUTES
}

pub mod limits {
    pub const MAX_PROJECTS_DEFAULT: u32 = 3;
    pub const MAX_PROJECTS_EXTRA: u32 = 15;
}

/// Max length of strings in the git metadata
pub const GIT_STRINGS_MAX_LENGTH: usize = 80;
/// Max HTTP body size for a deployment POST request
pub const CREATE_SERVICE_BODY_LIMIT: usize = 50_000_000;
pub const GIT_OPTION_NONE_TEXT: &str = "N/A";

pub const DEPLOYER_END_MSG_STARTUP_ERR: &str = "Service startup encountered an error";
pub const DEPLOYER_END_MSG_BUILD_ERR: &str = "Service build encountered an error";
pub const DEPLOYER_END_MSG_CRASHED: &str = "Service encountered an error and crashed";
pub const DEPLOYER_END_MSG_STOPPED: &str = "Service was stopped by the user"; // don't include this in end messages so that logs are not stopped too early
pub const DEPLOYER_END_MSG_COMPLETED: &str = "Service finished running all on its own";
pub const DEPLOYER_RUNTIME_START_RESPONSE: &str = "Runtime started successully";

pub const DEPLOYER_END_MESSAGES_BAD: &[&str] = &[
    DEPLOYER_END_MSG_STARTUP_ERR,
    DEPLOYER_END_MSG_BUILD_ERR,
    DEPLOYER_END_MSG_CRASHED,
];
pub const DEPLOYER_END_MESSAGES_GOOD: &[&str] =
    &[DEPLOYER_END_MSG_COMPLETED, DEPLOYER_RUNTIME_START_RESPONSE];
