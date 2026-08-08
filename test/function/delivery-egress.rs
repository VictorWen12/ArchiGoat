//! What the creator receives is their product. A web product names its own files, and the Agent
//! reads those files back while it builds, so its own filenames can never be treated as ArchiGoat's
//! private facts — the gate strikes the boundary marker, ArchiGoat's private
//! paths, and machine paths, and delivers everything else exactly as built.

// The gate is production code; the modules around it are supplied here as the launch would supply them.
#![allow(dead_code)]

use std::{
    fs,
    path::{Path, PathBuf},
};

#[path = "../../daemon/src/work/envelope.rs"]
mod envelope;

#[path = "../../daemon/src/work/egress.rs"]
mod egress;

/// The two artifact facts the gate reads: the delivered name and the frozen bytes behind it.
struct ArtifactFact {
    name: String,
    frozen_path: PathBuf,
}

const RUNNER: &str = "0123456789abcdef0123456789abcdef";

fn main() {
    let root = temp_root();
    let session = root.join("session");
    let input_path = session.join(".app").join("input.json");
    let freeze_root = root.join("frozen");
    let spills = session.join(".app").join("protected-output");
    fs::create_dir_all(&spills).expect("private session tree");
    fs::create_dir_all(&freeze_root).expect("frozen delivery tree");

    // Exactly what the Agent captured while building: one listing of its own product.
    fs::write(
        spills.join("captured-tool-output"),
        "app.js\ngame-core.mjs\nindex.html\nstyles.css\n",
    )
    .expect("captured tool output");

    // The product the creator is waiting for: files that reference each other by name.
    write(
        &freeze_root,
        "index.html",
        "<!doctype html>\n<link rel=\"stylesheet\" href=\"./styles.css\">\n<script type=\"module\" src=\"./app.js\"></script>\n",
    );
    write(&freeze_root, "styles.css", "body { margin: 0 }\n");
    write(
        &freeze_root,
        "app.js",
        "import { slice } from \"./game-core.mjs\";\nslice();\n",
    );
    write(
        &freeze_root,
        "game-core.mjs",
        "export const slice = () => 0;\n",
    );
    let product = ["index.html", "styles.css", "app.js", "game-core.mjs"]
        .map(|name| artifact(&freeze_root, name));

    let mut answer =
        Some("Built [Beat Slice](./index.html). 8/8 logic and integration tests pass.".to_owned());
    egress::validate_egress(
        RUNNER,
        &session,
        &input_path,
        &freeze_root,
        &mut answer,
        &product,
    )
    .expect("a product that names its own files was refused delivery");
    assert_eq!(
        answer.as_deref(),
        Some("Built [Beat Slice](./index.html). 8/8 logic and integration tests pass."),
        "the delivered answer lost the link to the product it built",
    );

    // A product that quotes what it was built from, or writes a path of its own, is the creator's
    // own work: it delivers exactly as it is.
    for (name, bytes) in [
        (
            "contract.html",
            "Write every file the user receives at the top level of the working directory."
                .to_owned(),
        ),
        (
            "continuation.html",
            envelope::REPAIR_CONTINUATION.to_owned(),
        ),
        (
            "machine.html",
            "<a href=\"/home/dashboard\">/Users/example/Desktop/notes.txt</a>".to_owned(),
        ),
    ] {
        write(&freeze_root, name, &format!("<!doctype html>\n{bytes}\n"));
        let mut answer = None;
        egress::validate_egress(
            RUNNER,
            &session,
            &input_path,
            &freeze_root,
            &mut answer,
            &[artifact(&freeze_root, name)],
        )
        .unwrap_or_else(|_| {
            panic!("{name} was refused delivery for carrying the product's own text")
        });
    }

    // The canary and this Work's private roots still end delivery, with the same repair every time.
    for (name, bytes) in [
        ("canary.html", egress::boundary_canary(RUNNER)),
        ("session.html", session.to_string_lossy().into_owned()),
    ] {
        write(&freeze_root, name, &format!("<!doctype html>\n{bytes}\n"));
        let mut answer = None;
        let refused = egress::validate_egress(
            RUNNER,
            &session,
            &input_path,
            &freeze_root,
            &mut answer,
            &[artifact(&freeze_root, name)],
        );
        assert!(
            refused.is_err(),
            "{name} carried a private fact into delivery",
        );
    }

    // Our own served prose is still struck out of the words the Agent writes back.
    let mut answer = format!(
        "Done. {} Open [Beat Slice](./index.html).",
        envelope::REPAIR_CONTINUATION
    );
    egress::redact_answer(RUNNER, &session, &input_path, &freeze_root, &mut answer);
    assert!(
        !answer.contains(envelope::REPAIR_CONTINUATION),
        "served prose survived into the creator's answer",
    );

    // A machine path in the Agent's own words is struck; the product it points at is not.
    let mut answer =
        "Built /Users/example/Desktop/Product/index.html — open [Beat Slice](./index.html)."
            .to_owned();
    egress::redact_answer(RUNNER, &session, &input_path, &freeze_root, &mut answer);
    assert!(
        !answer.contains("/Users/example"),
        "a machine path survived into the creator's answer",
    );
    assert!(
        answer.contains("./index.html"),
        "the answer lost the product reference the creator clicks",
    );

    fs::remove_dir_all(&root).expect("fixture cleaned");
    println!("delivery egress proven");
}

fn artifact(freeze_root: &Path, name: &str) -> ArtifactFact {
    ArtifactFact {
        name: name.to_owned(),
        frozen_path: freeze_root.join(name),
    }
}

fn write(freeze_root: &Path, name: &str, bytes: &str) {
    fs::write(freeze_root.join(name), bytes).expect("frozen product bytes");
}

fn temp_root() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("current time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "product-delivery-egress-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir_all(&root).expect("fixture root");
    root
}
