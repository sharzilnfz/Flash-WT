//! Doctor and store observability command handlers.

use std::env;

use wt_copy::probe_capabilities;
use wt_store::DiskStore;

use crate::config::RunConfig;
use crate::envelope::{
    Diagnostic, DoctorData, DoctorEnvVars, DoctorFsCapabilities, StoreDuData,
};
use crate::error::Result;
use crate::hydrate::store_dir;
use crate::output::HumanBytes;

fn env_var_opt(key: &str) -> Option<String> {
    env::var(key).ok()
}

fn collect_env_vars() -> DoctorEnvVars {
    DoctorEnvVars {
        wt_store: env_var_opt("WT_STORE"),
        wt_snapshots: env_var_opt("WT_SNAPSHOTS"),
        wt_snapshots_v2: env_var_opt("WT_SNAPSHOTS_V2"),
        wt_verify: env_var_opt("WT_VERIFY"),
        wt_timing: env_var_opt("WT_TIMING"),
        wt_gc_grace: env_var_opt("WT_GC_GRACE"),
        wt_snapshot_cap: env_var_opt("WT_SNAPSHOT_CAP"),
        wt_max_snapshot_bytes: env_var_opt("WT_MAX_SNAPSHOT_BYTES"),
        wt_hardlink: env_var_opt("WT_HARDLINK"),
        wt_no_hardlink: env_var_opt("WT_NO_HARDLINK"),
    }
}

pub fn run(cfg: &RunConfig) -> Result<(DoctorData, Vec<Diagnostic>)> {
    let resolved_store = store_dir()?;
    let env_vars = collect_env_vars();
    let fs_caps = probe_capabilities(&resolved_store);
    let store_usage = DiskStore::inspect_disk_usage(&resolved_store)?;

    let du_data = StoreDuData {
        store_path: resolved_store.display().to_string(),
        objects_bytes: store_usage.objects_bytes,
        snapshots_bytes: store_usage.snapshots_bytes,
        mirrors_bytes: store_usage.mirrors_bytes,
        refs_bytes: store_usage.refs_bytes,
        caches_bytes: store_usage.caches_bytes,
        total_bytes: store_usage.total_bytes,
    };

    let doctor_fs = DoctorFsCapabilities {
        apfs_clonefile: fs_caps.apfs_clonefile,
        ficlone: fs_caps.ficlone,
        copy_file_range: fs_caps.copy_file_range,
    };

    let data = DoctorData {
        store_path: resolved_store.display().to_string(),
        env_vars: env_vars.clone(),
        fs_capabilities: doctor_fs,
        store_disk_usage: du_data,
    };

    if !cfg.json {
        println!("Resolved Store:");
        println!("  path: {}", resolved_store.display());
        println!();
        println!("Environment Variables:");
        let env_pairs = [
            ("WT_STORE", env_vars.wt_store.as_deref()),
            ("WT_SNAPSHOTS", env_vars.wt_snapshots.as_deref()),
            ("WT_SNAPSHOTS_V2", env_vars.wt_snapshots_v2.as_deref()),
            ("WT_VERIFY", env_vars.wt_verify.as_deref()),
            ("WT_TIMING", env_vars.wt_timing.as_deref()),
            ("WT_GC_GRACE", env_vars.wt_gc_grace.as_deref()),
            ("WT_SNAPSHOT_CAP", env_vars.wt_snapshot_cap.as_deref()),
            ("WT_MAX_SNAPSHOT_BYTES", env_vars.wt_max_snapshot_bytes.as_deref()),
            ("WT_HARDLINK", env_vars.wt_hardlink.as_deref()),
            ("WT_NO_HARDLINK", env_vars.wt_no_hardlink.as_deref()),
        ];
        for (k, v) in env_pairs {
            match v {
                Some(val) => println!("  {k} = {val}"),
                None => println!("  {k} = (unset)"),
            }
        }
        println!();
        println!("Filesystem Capabilities:");
        println!(
            "  APFS clonefile: {}",
            if fs_caps.apfs_clonefile {
                "supported"
            } else {
                "unsupported"
            }
        );
        println!(
            "  FICLONE: {}",
            if fs_caps.ficlone {
                "supported"
            } else {
                "unsupported"
            }
        );
        println!(
            "  copy_file_range: {}",
            if fs_caps.copy_file_range {
                "supported"
            } else {
                "unsupported"
            }
        );
        println!();
        println!("Store Disk Usage:");
        println!(
            "  objects:   {} ({} bytes)",
            HumanBytes(store_usage.objects_bytes),
            store_usage.objects_bytes
        );
        println!(
            "  snapshots: {} ({} bytes)",
            HumanBytes(store_usage.snapshots_bytes),
            store_usage.snapshots_bytes
        );
        println!(
            "  mirrors:   {} ({} bytes)",
            HumanBytes(store_usage.mirrors_bytes),
            store_usage.mirrors_bytes
        );
        println!(
            "  refs:      {} ({} bytes)",
            HumanBytes(store_usage.refs_bytes),
            store_usage.refs_bytes
        );
        println!(
            "  caches:    {} ({} bytes)",
            HumanBytes(store_usage.caches_bytes),
            store_usage.caches_bytes
        );
        println!(
            "  total:     {} ({} bytes)",
            HumanBytes(store_usage.total_bytes),
            store_usage.total_bytes
        );
    }

    Ok((data, Vec::new()))
}

pub fn store_du(cfg: &RunConfig) -> Result<(StoreDuData, Vec<Diagnostic>)> {
    let resolved_store = store_dir()?;
    let store_usage = DiskStore::inspect_disk_usage(&resolved_store)?;

    let data = StoreDuData {
        store_path: resolved_store.display().to_string(),
        objects_bytes: store_usage.objects_bytes,
        snapshots_bytes: store_usage.snapshots_bytes,
        mirrors_bytes: store_usage.mirrors_bytes,
        refs_bytes: store_usage.refs_bytes,
        caches_bytes: store_usage.caches_bytes,
        total_bytes: store_usage.total_bytes,
    };

    if !cfg.json {
        println!("Store disk usage for {}:", resolved_store.display());
        println!(
            "  objects:   {} ({} bytes)",
            HumanBytes(store_usage.objects_bytes),
            store_usage.objects_bytes
        );
        println!(
            "  snapshots: {} ({} bytes)",
            HumanBytes(store_usage.snapshots_bytes),
            store_usage.snapshots_bytes
        );
        println!(
            "  mirrors:   {} ({} bytes)",
            HumanBytes(store_usage.mirrors_bytes),
            store_usage.mirrors_bytes
        );
        println!(
            "  refs:      {} ({} bytes)",
            HumanBytes(store_usage.refs_bytes),
            store_usage.refs_bytes
        );
        println!(
            "  caches:    {} ({} bytes)",
            HumanBytes(store_usage.caches_bytes),
            store_usage.caches_bytes
        );
        println!(
            "  total:     {} ({} bytes)",
            HumanBytes(store_usage.total_bytes),
            store_usage.total_bytes
        );
    }

    Ok((data, Vec::new()))
}
