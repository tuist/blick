//! Per-command modules. Each command has a single public `run` entry point;
//! [`crate::app`] dispatches to them based on the parsed CLI.

pub mod init;
pub mod learn_cmd;
pub mod publish_cmd;
pub mod render_cmd;
pub mod review;
pub mod show_config;
