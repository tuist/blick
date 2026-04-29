//! `blick init` — write a starter `blick.toml`.

use std::fs;

use crate::cli::InitArgs;
use crate::config::ConfigFile;
use crate::error::BlickError;

pub fn run(args: InitArgs) -> Result<(), BlickError> {
    if args.path.exists() && !args.force {
        return Err(BlickError::Config(format!(
            "{} already exists. Re-run with --force to overwrite it.",
            args.path.display()
        )));
    }

    let config = ConfigFile::starter(args.agent, args.model);
    fs::write(&args.path, config.to_toml()?)?;
    println!("Created {}", args.path.display());
    Ok(())
}
