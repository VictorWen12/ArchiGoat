//! Model discovery reads each Provider's own catalog; ArchiGoat curates no list of its own.

use crate::provider::{LocalCli, ModelChoice, ModelSource, Provider};

/// A catalog read waits on no human, so it reaches a conclusion inside this bound.
const CATALOG_SECONDS: u64 = 20;

/// Discover returns the Provider's own current catalog, or nothing when it stays silent.
pub(crate) async fn discover(provider: Provider, program: &LocalCli) -> Vec<ModelChoice> {
    match provider.model_source() {
        ModelSource::Fixed(models) => models,
        ModelSource::Command(args) => {
            match crate::cli::capture(program, &args, CATALOG_SECONDS).await {
                Ok(output) => provider.model_catalog(&output.stdout),
                Err(_) => Vec::new(),
            }
        }
        ModelSource::Dialogue {
            args,
            input,
            finished,
        } => match crate::cli::dialogue(program, &args, &input, finished, CATALOG_SECONDS).await {
            Ok(answer) => provider.model_catalog(&answer),
            Err(_) => Vec::new(),
        },
    }
}
