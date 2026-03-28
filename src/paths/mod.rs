mod app_home;
mod cache;

pub use app_home::*;
pub use cache::*;

pub const APP_HOME_ENV_VAR: &str = "LOCATE_GIT_PROJECTS_ON_MY_COMPUTER_HOME_DIR";
pub const APP_HOME_DIR_NAME: &str = "locate-git-projects-on-my-computer";

pub const APP_CACHE_ENV_VAR: &str = "LOCATE_GIT_PROJECTS_ON_MY_COMPUTER_CACHE_DIR";
pub const APP_CACHE_DIR_NAME: &str = "locate-git-projects-on-my-computer";
