//! A message the user sends mid-run is their own turn: the launch that carries it delivers those exact
//! words into the live session framed like the first turn. It is said once. A runner replaced after the
//! Agent already received those words resumes that same turn instead of asking for the work twice.

#[path = "../../daemon/src/work/envelope.rs"]
mod envelope;

fn main() {
    let first = r#"{"conversation":"c1","goal":"Build a runner game","context":[],"resume":false,"attachments":[]}"#;
    let steer = r#"{"id":"a1","conversation":"c1","goal":"make the goat jump twice","context":[],"attachments":[],"resume":true}"#;

    // A follow-up sent mid-turn relaunches the same session carrying the user's own words.
    let delivery = envelope::select(first, Some(steer), false, false, false);
    assert_eq!(
        delivery.envelope, steer,
        "a steer relaunch dropped the user's message"
    );
    assert!(
        delivery.envelope.contains("make the goat jump twice"),
        "a steer relaunch rewrote the user's words",
    );
    assert!(delivery.resume, "a steer relaunch started a new session");
    assert!(
        delivery.request,
        "a steer relaunch lost its staged attachment paths",
    );

    // A steer turn that died before any runner heard those words still delivers them: nothing was said.
    let delivery = envelope::select(first, Some(steer), false, true, false);
    assert_eq!(
        delivery.envelope, steer,
        "a repair replaced a message the Agent never received with a repair blob",
    );
    assert!(delivery.resume, "a repaired steer left its live session");
    assert!(
        delivery.request,
        "a repaired steer lost its staged attachment paths",
    );

    // A steer turn that died after the Agent received those words continues instead of repeating them:
    // the Agent already acted on that instruction, and one message is one instruction.
    let delivery = envelope::select(first, Some(steer), true, true, false);
    assert_eq!(
        delivery.envelope,
        envelope::REPAIR_CONTINUATION,
        "repair said the user's message to the Agent a second time",
    );
    assert!(
        !delivery.envelope.contains("make the goat jump twice"),
        "a repair continuation carried the user's already-delivered words",
    );
    assert!(delivery.resume, "a repaired steer left its live session");
    assert!(
        !delivery.request,
        "repair continuation was delivered as a stored request envelope",
    );

    // A delivered follow-up that is not being repaired is still the live turn's own message.
    let delivery = envelope::select(first, Some(steer), true, false, false);
    assert_eq!(
        delivery.envelope, steer,
        "a follow-up lost the user's words to a launch that was never a repair",
    );

    // Repair with nothing queued continues the session without repeating the creator's request.
    let delivery = envelope::select(first, None, false, true, false);
    assert_eq!(
        delivery.envelope,
        envelope::REPAIR_CONTINUATION,
        "repair delivered something other than a plain continuation",
    );
    assert_ne!(
        delivery.envelope, first,
        "repair replayed the creator's request into a live session",
    );
    assert!(delivery.resume, "repair started a new session");
    // A continuation names no attachment path, so no launch may re-read it as the stored JSON request.
    assert!(
        !delivery.request,
        "repair continuation was delivered as a stored request envelope",
    );
    assert!(
        serde_free_json_object(envelope::REPAIR_CONTINUATION).is_none(),
        "repair continuation looks like JSON and would be parsed as one",
    );

    // A first turn delivers the creator's untouched request and resumes only a bound session.
    let delivery = envelope::select(first, None, false, false, false);
    assert_eq!(
        (delivery.envelope, delivery.resume, delivery.request),
        (first, false, true),
        "a first turn changed the creator's request",
    );
    let delivery = envelope::select(first, None, false, false, true);
    assert_eq!(
        (delivery.envelope, delivery.resume),
        (first, true),
        "a bound conversation lost its resume",
    );

    // No launch may reject the creator's own follow-up: the envelope module holds no framing gate,
    // and no launch stamps the model's own words as data it must distrust.
    let module = include_str!("../../daemon/src/work/envelope.rs");
    assert!(
        !module.contains("STEER_FRAMING") && !module.contains("lost the user's turn framing"),
        "a steer framing gate returned to the envelope module",
    );
    let runtime = include_str!("../../daemon/src/work/runtime.rs");
    assert!(
        !runtime.contains("untrusted") && !runtime.contains("AUTHORITY"),
        "the launch envelope stamps authority or untrusted labeling again",
    );

    println!("steer envelope proven");
}

/// A dependency-free check that some bytes open a JSON object, the only shape carrying attachment paths.
fn serde_free_json_object(value: &str) -> Option<&str> {
    value.trim_start().strip_prefix('{')
}
