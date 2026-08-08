//! A reattached observer trusts only the exact kernel lock held by its runner process.

#[path = "../../daemon/src/windows/liveness.rs"]
mod liveness;

use std::{
    fs,
    io::{BufRead, Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--hold") {
        let claim = PathBuf::from(std::env::args().nth(2).expect("claim path"));
        let _owner = liveness::RunnerLiveness::claim(&claim)
            .expect("runner claims liveness")
            .expect("runner owns new claim");
        println!("held");
        std::io::stdout().flush().expect("holder announces claim");
        std::io::stdin()
            .read_to_end(&mut Vec::new())
            .expect("holder waits");
        return;
    }

    let claim = claim_path();
    let executable = std::env::current_exe().expect("test executable");
    let mut runner = Command::new(executable)
        .arg("--hold")
        .arg(&claim)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("runner starts");
    let mut ready = String::new();
    std::io::BufReader::new(runner.stdout.take().expect("runner output"))
        .read_line(&mut ready)
        .expect("runner claim arrives");
    assert_eq!(ready, "held\n");

    let reattached = liveness::RunnerLiveness::reattach(&claim)
        .expect("liveness inspection")
        .expect("living runner reattaches");
    assert!(reattached.is_live(), "held kernel object proves the runner");

    runner.kill().expect("runner dies");
    runner.wait().expect("runner reaped");
    assert!(
        !reattached.is_live(),
        "dead runner cannot renew through stale state"
    );
    assert!(
        liveness::RunnerLiveness::reattach(&claim)
            .expect("dead liveness inspection")
            .is_none(),
        "dead runner cannot reacquire through its durable identity",
    );
    assert!(
        liveness::RunnerLiveness::claim(&claim)
            .expect("replay claim inspection")
            .is_none(),
        "durable claim rejects runner replay after death",
    );
    drop(reattached);
    fs::remove_file(claim).expect("claim proof cleaned");
}

fn claim_path() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("current time")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "product-runner-liveness-{}-{unique}.claim",
        std::process::id()
    ))
}
