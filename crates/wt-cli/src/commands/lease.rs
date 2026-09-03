//! `wt lease show`: Machine-readable scratch lease inspection (ticket 11).

use std::time::{SystemTime, UNIX_EPOCH};

use wt_store::{is_lease_expired, is_process_alive, read_leases};

use crate::cli::LeaseAction;
use crate::config::RunConfig;
use crate::envelope::{Diagnostic, LeaseData, LeaseEntry};
use crate::error::{Error, Result};
use crate::hydrate::open_store;
use crate::output::{HumanDuration, format_table};

pub fn run(action: Option<LeaseAction>, cfg: &RunConfig) -> Result<(LeaseData, Vec<Diagnostic>)> {
    let (target_id, show_all) = match action {
        Some(LeaseAction::Show { id, all }) => (id, all),
        None => (None, false),
    };

    let store = open_store()?;
    let raw_leases = read_leases(store.root());
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut entries = Vec::new();

    for read_lease in raw_leases {
        if let Ok(lease) = read_lease.lease {
            let pid_alive = is_process_alive(lease.pid, lease.start_time);
            let expired = is_lease_expired(&lease);
            let ttl_remaining = lease.expires_at.saturating_sub(now_secs);

            if !show_all && target_id.is_none() && (expired || !pid_alive) {
                continue;
            }

            entries.push(LeaseEntry {
                lease_id: lease.id.clone(),
                pid: lease.pid,
                pid_alive,
                expires_at: lease.expires_at,
                ttl_remaining_secs: ttl_remaining,
                is_expired: expired,
                worktree_path: Some(lease.worktree.display().to_string()),
                git_dir: Some(lease.gitdir.display().to_string()),
            });
        }
    }

    let (final_leases, matched) = if let Some(id) = target_id {
        let matched = entries
            .into_iter()
            .find(|e| {
                e.lease_id == id
                    || format!("scratch-{}", e.lease_id) == id
                    || id.strip_prefix("scratch-") == Some(&e.lease_id)
            })
            .ok_or_else(|| Error::Usage(format!("lease '{id}' not found")))?;
        (vec![matched.clone()], Some(matched))
    } else {
        (entries, None)
    };

    if !cfg.json {
        print_human_leases(&final_leases);
    }

    Ok((
        LeaseData {
            leases: final_leases,
            matched_lease: matched,
        },
        Vec::new(),
    ))
}

fn print_human_leases(leases: &[LeaseEntry]) {
    if leases.is_empty() {
        println!("No active scratch leases found.");
        return;
    }

    let mut rows = Vec::new();
    for l in leases {
        let status = if l.is_expired {
            "expired".to_string()
        } else if l.pid_alive {
            "alive".to_string()
        } else {
            "dead".to_string()
        };
        let ttl = if l.is_expired {
            "0s".to_string()
        } else {
            format!("{}", HumanDuration(l.ttl_remaining_secs))
        };
        let worktree = l.worktree_path.as_deref().unwrap_or("-");

        rows.push(vec![
            l.lease_id.clone(),
            l.pid.to_string(),
            status,
            ttl,
            worktree.to_string(),
        ]);
    }

    println!(
        "{}",
        format_table(
            &["LEASE ID", "PID", "STATUS", "TTL REMAINING", "WORKTREE"],
            &rows
        )
    );
}
