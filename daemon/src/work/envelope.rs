//! One launch delivers exactly one envelope, and the user's own words outrank every daemon continuation.

/// Repair continues the same native session; the model is never handed an action to invent.
pub(crate) const REPAIR_CONTINUATION: &str =
    "Continue this session from where the previous turn ended.";

/// Delivery is the exact bytes one launch carries plus the two facts that launch needs about them.
pub(crate) struct Delivery<'a> {
    /// Envelope is what the native session receives on this launch.
    pub(crate) envelope: &'a str,
    /// Resume proves this launch continues the bound native session instead of opening one.
    pub(crate) resume: bool,
    /// Request is true only for a stored JSON envelope, the one shape carrying staged attachment paths.
    pub(crate) request: bool,
}

/// Select returns the exact bytes this launch delivers.
/// A queued follow-up outranks repair, so the user's message survives rotation until a launch carries it.
/// Once a launch has carried it, no later launch says it again: the Agent heard it, and a replacement
/// runner resumes the same conversation with a plain continuation.
pub(crate) fn select<'a>(
    original: &'a str,
    steer: Option<&'a str>,
    delivered: bool,
    repair: bool,
    resume: bool,
) -> Delivery<'a> {
    match (steer, repair) {
        (Some(_), true) if delivered => Delivery {
            envelope: REPAIR_CONTINUATION,
            resume: true,
            request: false,
        },
        (Some(steer), _) => Delivery {
            envelope: steer,
            resume: true,
            request: true,
        },
        (None, true) => Delivery {
            envelope: REPAIR_CONTINUATION,
            resume: true,
            request: false,
        },
        (None, false) => Delivery {
            envelope: original,
            resume,
            request: true,
        },
    }
}
