//! One conversation keeps its latest delivered app, and no other conversation can inherit it.

#[path = "../../daemon/src/work/app.rs"]
mod app;

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    let root = std::env::temp_dir().join(format!(
        "archigoat-edit-in-place-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::create_dir_all(&root).expect("test root");

    let store = root.join("Apps/chat-one");
    let first_freeze = root.join("freeze-one");
    fs::create_dir_all(&first_freeze).expect("first freeze");
    write(&first_freeze, "index.html", "<button>blue</button>");
    write(&first_freeze, "styles.css", "button { color: blue }");
    app::replace(&store, &first_freeze, &["index.html", "styles.css"])
        .expect("first delivery kept");

    let first_work = root.join("work-one");
    fs::create_dir_all(&first_work).expect("first work");
    app::seed(&store, &first_work).expect("first edit seeded");
    assert_eq!(
        read(&first_work.join("index.html")),
        "<button>blue</button>"
    );
    assert_eq!(
        read(&first_work.join("styles.css")),
        "button { color: blue }"
    );

    let second_freeze = root.join("freeze-two");
    fs::create_dir_all(&second_freeze).expect("second freeze");
    write(&second_freeze, "index.html", "<button>red</button>");
    write(&second_freeze, "app.js", "document.title = 'edited';");
    app::replace(&store, &second_freeze, &["index.html", "app.js"])
        .expect("second delivery replaced");

    let second_work = root.join("work-two");
    fs::create_dir_all(&second_work).expect("second work");
    app::seed(&store, &second_work).expect("second edit seeded");
    assert_eq!(
        read(&second_work.join("index.html")),
        "<button>red</button>"
    );
    assert_eq!(
        read(&second_work.join("app.js")),
        "document.title = 'edited';"
    );
    assert!(
        !second_work.join("styles.css").exists(),
        "stale file survived replacement"
    );

    let other_work = root.join("work-other");
    fs::create_dir_all(&other_work).expect("other work");
    app::seed(&root.join("Apps/chat-two"), &other_work).expect("other conversation is empty");
    assert!(
        is_empty(&other_work),
        "another conversation inherited the app"
    );

    fs::remove_dir_all(&root).expect("test root removed");
    println!("edit in place proven");
}

fn write(root: &Path, name: &str, bytes: &str) {
    fs::write(root.join(name), bytes).expect("fixture file");
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).expect("stored file")
}

fn is_empty(path: &Path) -> bool {
    fs::read_dir(path).expect("workspace").next().is_none()
}
