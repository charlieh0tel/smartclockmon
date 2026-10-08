//! Elsewhere than Linux: no sensors of the platform's own.

use crate::sensor::ConfigError;
use crate::sensor::Source;

/// No switches.
#[derive(Debug, clap::Args)]
pub struct Args {}

pub(super) fn sources(_: &Args) -> Result<Vec<Box<dyn Source>>, ConfigError> {
    Ok(Vec::new())
}
