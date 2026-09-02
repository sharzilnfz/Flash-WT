//! `wt init`: Initialize a starter `.wtinclude` manifest (Ticket 09).

use std::path::Path;

use crate::config::RunConfig;
use crate::envelope::{Diagnostic, InitData};
use crate::error::{Error, Result};
use crate::hydration_filter::write_starter_manifest;
use crate::workspace::WorkspaceEngine;

pub fn run(
    dir: Option<&Path>,
    force: bool,
    cfg: &RunConfig,
) -> Result<(InitData, Vec<Diagnostic>)> {
    let target_dir = if let Some(d) = dir {
        d.to_path_buf()
    } else {
        let engine = WorkspaceEngine::discover()?;
        engine.root().to_path_buf()
    };

    let manifest_path = if target_dir.file_name().and_then(|n| n.to_str()) == Some(".wtinclude") {
        target_dir
    } else {
        target_dir.join(".wtinclude")
    };

    if manifest_path.exists() && !force {
        return Err(Error::Usage(format!(
            "manifest {} already exists (use --force to overwrite)",
            manifest_path.display()
        )));
    }

    write_starter_manifest(&manifest_path)?;

    if !cfg.json {
        println!("wrote starter manifest {}", manifest_path.display());
    }

    let data = InitData {
        manifest_path: manifest_path.display().to_string(),
        created: true,
    };

    Ok((data, Vec::new()))
}
