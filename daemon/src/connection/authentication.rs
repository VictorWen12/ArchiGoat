//! Native authentication converts official CLI evidence into explicit connection facts.

use crate::provider::{LocalCli, Provider};

/// Authentication separates explicit login facts from observation uncertainty.
pub(crate) enum Authentication {
    Authenticated,
    SignedOut,
    Unavailable,
    CannotStart,
}

/// Authentication reports one official CLI observation without inventing a retry gate.
pub(crate) async fn authentication(provider: Provider, program: &LocalCli) -> Authentication {
    match crate::cli::capture(program, &provider.auth_status_args(), 20).await {
        Ok(output) => {
            // Claude's evidence is a strict whole-string JSON parse, so a stray stderr byte must not poison it.
            let evidence = match provider {
                Provider::Claude => output.stdout.clone(),
                _ => format!("{}{}", output.stdout, output.stderr),
            };
            // A denial is read first because CLIs print it on a successful exit, where a lenient
            // affirmation would otherwise claim the user's signed-out account is connected.
            match () {
                _ if provider.signed_out(&evidence) => Authentication::SignedOut,
                _ if provider.authenticated(output.success, &evidence) => {
                    Authentication::Authenticated
                }
                _ => Authentication::Unavailable,
            }
        }
        Err(_) => Authentication::CannotStart,
    }
}
