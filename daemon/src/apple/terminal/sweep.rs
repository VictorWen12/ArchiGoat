//! Sweep ends the process tree owned by a finishing Work.

use std::process::Stdio;
use tokio::process::Command;

/// TreeSurvivors lists the leader's live descendants before its process group ends.
pub(super) async fn tree_survivors(group: u32) -> Vec<u32> {
    let mut survivors = Vec::new();
    // The parent chain finds descendants that left the process group while it still stands.
    // A Work's descendants always run as the Work's own user, so this listing never reaches another account's processes.
    if let Some(listing) = processes(&["-xo", "pid=,ppid="]).await {
        let mut edges = Vec::new();
        for line in listing.lines() {
            let mut fields = line.split_whitespace();
            if let (Some(pid), Some(parent)) = (parse_pid(fields.next()), parse_pid(fields.next()))
            {
                edges.push((pid, parent));
            }
        }
        let mut owned = vec![group];
        let mut index = 0;
        while index < owned.len() {
            let parent = owned[index];
            index += 1;
            for (pid, candidate) in &edges {
                if *candidate == parent && !owned.contains(pid) {
                    owned.push(*pid);
                }
            }
        }
        survivors.extend(owned.into_iter().skip(1));
    }
    survivors
}

/// Ends the listed processes, tolerating members an earlier group kill already reaped.
/// PHYSICS: these are orphans of a process group whose own parent chain is already gone.
pub(super) async fn end_processes(pids: &[u32]) {
    if pids.is_empty() {
        return;
    }
    let mut kill = Command::new("/bin/kill");
    kill.arg("-KILL").arg("--");
    for pid in pids {
        kill.arg(pid.to_string());
    }
    let _ = kill
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
}

// One process listing tolerating platforms or states where the flags fail.
async fn processes(args: &[&str]) -> Option<String> {
    let output = Command::new("/bin/ps")
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

// Pid fields parse defensively because ps output is formatted for humans first.
fn parse_pid(field: Option<&str>) -> Option<u32> {
    field?.parse::<u32>().ok().filter(|pid| *pid > 1)
}
