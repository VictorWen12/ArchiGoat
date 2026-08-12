//! Done delivery streams exact frozen artifacts before committing their ordered Account products.

use std::collections::HashMap;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;

use crate::{
    DaemonState,
    state::{RunPhase, RunSnapshot},
};

/// AccountReceipt proves Account stored the exact frozen artifact sent by this ArchiGoat.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountReceipt {
    product_id: String,
    work_id: String,
    name: String,
    form: String,
    exact_bytes: u64,
    sha256: String,
}

/// Presign grants one exact direct upload without exposing storage credentials.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Presign {
    upload_url: String,
    headers: HashMap<String, String>,
}

/// ProductRef preserves snapshot artifact order in the terminal delivery.
#[derive(Serialize)]
struct ProductRef {
    id: String,
}

/// DeliveryRequest carries only verified Done output and Account-owned product identities.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryRequest<'a> {
    work_id: &'a str,
    answer: &'a str,
    products: Vec<ProductRef>,
}

/// LocalDelivery carries only the exact browser-owned destination for one local Work handoff.
#[derive(Clone, Copy)]
pub(super) struct LocalDelivery<'a> {
    pub(super) delivery_id: &'a str,
    pub(super) scope_kind: &'a str,
    pub(super) scope_id: &'a str,
}

/// LocalDeliveryRequest commits a local Work through Account's owner-authenticated path.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalDeliveryRequest<'a> {
    scope_kind: &'a str,
    scope_id: &'a str,
    delivery_id: &'a str,
    work_id: &'a str,
    answer: &'a str,
    products: Vec<ProductRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    design_mock: Option<crate::local::DesignMock>,
}

/// DeliveryReceipt proves Account committed this exact Work result.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeliveryReceipt {
    work_id: String,
    product_ids: Vec<String>,
    #[serde(rename = "turnIds")]
    _turn_ids: Vec<i64>,
}

/// Artifact requests bind one authenticated ArchiGoat receipt to every ingest step.
fn artifact_request(
    client: &Client,
    state: &DaemonState,
    credential: &str,
    path: &str,
    download: &crate::api::artifact::Download,
    local: bool,
) -> reqwest::RequestBuilder {
    let request = client.post(super::endpoint(state, path));
    let request = if local {
        super::daemon_request(request.bearer_auth(credential), state)
    } else {
        super::authorized_request(request, state, credential)
    };
    request
        .timeout(super::STREAM_TIMEOUT)
        .header("content-type", &download.content_type)
        .header("x-work-id", &download.work_id)
        .header("x-work-artifact-id", &download.artifact_id)
        .header("x-work-artifact-bytes", download.content_length)
        .header("x-work-artifact-sha256", &download.sha256)
        .header("x-file-name", &download.encoded_name)
        .header("x-file-title", &download.encoded_title)
        .header("x-file-tags", &download.encoded_tags)
}

/// Send retries safely because Account artifact and Work delivery identities are deterministic.
pub(super) async fn send(
    client: &Client,
    state: &DaemonState,
    credential: &str,
    work_id: &str,
    snapshot: &RunSnapshot,
) -> Result<(), String> {
    send_to(
        client,
        state,
        work_id,
        snapshot,
        DeliveryMode::Remote { credential },
    )
    .await
}

/// SendLocal uses the same frozen-byte delivery loop with the shell's Account session.
pub(super) async fn send_local(
    client: &Client,
    state: &DaemonState,
    account_token: &str,
    work_id: &str,
    snapshot: &RunSnapshot,
    destination: LocalDelivery<'_>,
) -> Result<(), String> {
    send_to(
        client,
        state,
        work_id,
        snapshot,
        DeliveryMode::Local {
            account_token,
            destination,
        },
    )
    .await
}

#[derive(Clone, Copy)]
enum DeliveryMode<'a> {
    Remote {
        credential: &'a str,
    },
    Local {
        account_token: &'a str,
        destination: LocalDelivery<'a>,
    },
}

impl DeliveryMode<'_> {
    fn credential(&self) -> &str {
        match self {
            Self::Remote { credential } => credential,
            Self::Local { account_token, .. } => account_token,
        }
    }

    fn local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }
}

async fn rejection(
    state: &DaemonState,
    mode: DeliveryMode<'_>,
    status: reqwest::StatusCode,
    context: &str,
) -> String {
    match mode {
        DeliveryMode::Remote { credential } => {
            super::rejected(state, credential, status, context).await
        }
        DeliveryMode::Local { .. } => format!("Account rejected {context} with {status}"),
    }
}

async fn send_to(
    client: &Client,
    state: &DaemonState,
    work_id: &str,
    snapshot: &RunSnapshot,
    mode: DeliveryMode<'_>,
) -> Result<(), String> {
    if snapshot.phase != RunPhase::Done {
        return Err("Work is not Done".to_owned());
    }
    let mut product_ids = Vec::with_capacity(snapshot.files.len());

    for file in &snapshot.files {
        let run = snapshot
            .run
            .as_deref()
            .filter(|run| !run.is_empty())
            .ok_or_else(|| "Done artifact run is missing".to_owned())?;
        if file.work_id != work_id {
            return Err("Done artifact belongs to another Work".to_owned());
        }
        let download = crate::api::artifact::download(state, work_id, run, &file.name)
            .map_err(|_| "Done artifact bytes are unavailable".to_owned())?;
        if download.work_id != file.work_id
            || download.artifact_id != file.artifact_id
            || download.content_length != file.bytes
            || download.sha256 != file.sha256
            || download.content_type != file.format
        {
            return Err("Done artifact receipt changed".to_owned());
        }
        let expected = AccountReceipt {
            product_id: String::new(),
            work_id: download.work_id.clone(),
            name: file.name.clone(),
            form: download.content_type.clone(),
            exact_bytes: download.content_length,
            sha256: download.sha256.clone(),
        };
        let receipt = if mode.local() {
            let response = artifact_request(
                client,
                state,
                mode.credential(),
                "/auth/work/artifact",
                &download,
                true,
            )
            .header("content-length", download.content_length)
            .body(reqwest::Body::wrap_stream(ReaderStream::new(
                tokio::fs::File::from_std(download.file),
            )))
            .send()
            .await
            .map_err(super::network)?;
            let status = response.status();
            if !status.is_success() {
                return Err(rejection(state, mode, status, "Work artifact").await);
            }
            response
                .json::<AccountReceipt>()
                .await
                .map_err(|_| "Account artifact receipt is invalid".to_owned())?
        } else {
            let presign = artifact_request(
                client,
                state,
                mode.credential(),
                "/auth/app/work/artifact/presign",
                &download,
                false,
            )
            .timeout(super::METADATA_TIMEOUT)
            .send()
            .await
            .map_err(super::network)?;
            if presign.status() == reqwest::StatusCode::GONE {
                eprintln!("Product released Work {work_id}: its owner no longer holds it");
                return Ok(());
            }
            if presign.status() == reqwest::StatusCode::NOT_FOUND {
                let response = artifact_request(
                    client,
                    state,
                    mode.credential(),
                    "/auth/app/work/artifact",
                    &download,
                    false,
                )
                .header("content-length", download.content_length)
                .body(reqwest::Body::wrap_stream(ReaderStream::new(
                    tokio::fs::File::from_std(download.file),
                )))
                .send()
                .await
                .map_err(super::network)?;
                let status = response.status();
                if status == reqwest::StatusCode::GONE {
                    eprintln!("Product released Work {work_id}: its owner no longer holds it");
                    return Ok(());
                }
                if !status.is_success() {
                    return Err(rejection(state, mode, status, "Work artifact").await);
                }
                response
                    .json::<AccountReceipt>()
                    .await
                    .map_err(|_| "Account artifact receipt is invalid".to_owned())?
            } else {
                let status = presign.status();
                if !status.is_success() {
                    return Err(rejection(state, mode, status, "Work artifact presign").await);
                }
                let presign = presign
                    .json::<Presign>()
                    .await
                    .map_err(|_| "Account artifact presign is invalid".to_owned())?;
                let upload_url = url::Url::parse(&presign.upload_url)
                    .map_err(|_| "Account artifact upload URL is invalid".to_owned())?;
                if upload_url.scheme() != "https"
                    || upload_url.origin().ascii_serialization() != state.config.artifact_origin
                    || !upload_url.username().is_empty()
                    || upload_url.password().is_some()
                    || upload_url.fragment().is_some()
                {
                    return Err("Account artifact upload URL origin is not trusted".to_owned());
                }
                let mut upload = client.put(upload_url).timeout(super::STREAM_TIMEOUT);
                for (name, value) in presign.headers {
                    upload = upload.header(name, value);
                }
                let complete = artifact_request(
                    client,
                    state,
                    mode.credential(),
                    "/auth/app/work/artifact/complete",
                    &download,
                    false,
                )
                .timeout(super::METADATA_TIMEOUT);
                let response = upload
                    .body(reqwest::Body::wrap_stream(ReaderStream::new(
                        tokio::fs::File::from_std(download.file),
                    )))
                    .send()
                    .await
                    .map_err(|error| format!("Direct artifact upload failed: {error}"))?;
                let status = response.status();
                if !status.is_success() {
                    return Err(format!("Direct artifact upload returned {status}"));
                }
                let response = complete.send().await.map_err(super::network)?;
                let status = response.status();
                if status == reqwest::StatusCode::GONE {
                    eprintln!("Product released Work {work_id}: its owner no longer holds it");
                    return Ok(());
                }
                if !status.is_success() {
                    return Err(rejection(state, mode, status, "Work artifact completion").await);
                }
                response
                    .json::<AccountReceipt>()
                    .await
                    .map_err(|_| "Account artifact receipt is invalid".to_owned())?
            }
        };
        if receipt.product_id.is_empty()
            || receipt.work_id != expected.work_id
            || receipt.name != expected.name
            || receipt.form != expected.form
            || receipt.exact_bytes != expected.exact_bytes
            || receipt.sha256 != expected.sha256
        {
            return Err("Account artifact receipt does not match".to_owned());
        }
        product_ids.push(receipt.product_id);
    }

    let design_mock = if mode.local() {
        crate::local::design_mock(state, work_id).await?
    } else {
        None
    };
    let uploaded = product_ids.len();
    let response = match mode {
        DeliveryMode::Remote { credential } => super::authorized_request(
            client.post(super::endpoint(state, "/auth/app/work/deliver")),
            state,
            credential,
        )
        .json(&DeliveryRequest {
            work_id,
            answer: &snapshot.text,
            products: product_ids
                .into_iter()
                .map(|id| ProductRef { id })
                .collect(),
        })
        .timeout(super::METADATA_TIMEOUT)
        .send()
        .await
        .map_err(super::network)?,
        DeliveryMode::Local {
            account_token,
            destination,
        } => super::daemon_request(
            client
                .post(super::endpoint(state, "/auth/work/deliver"))
                .bearer_auth(account_token),
            state,
        )
        .json(&LocalDeliveryRequest {
            scope_kind: destination.scope_kind,
            scope_id: destination.scope_id,
            delivery_id: destination.delivery_id,
            work_id,
            answer: &snapshot.text,
            products: product_ids
                .into_iter()
                .map(|id| ProductRef { id })
                .collect(),
            design_mock,
        })
        .timeout(super::METADATA_TIMEOUT)
        .send()
        .await
        .map_err(super::network)?,
    };
    let status = response.status();
    if status == reqwest::StatusCode::GONE {
        eprintln!("Product released Work {work_id}: its owner no longer holds it");
        return Ok(());
    }
    if !status.is_success() {
        return Err(rejection(state, mode, status, "Work delivery").await);
    }
    let receipt = response
        .json::<DeliveryReceipt>()
        .await
        .map_err(|_| "Account Work receipt is invalid".to_owned())?;
    let products_match = if uploaded == 0 {
        receipt.product_ids.is_empty()
    } else {
        receipt.product_ids.len() == 1 && !receipt.product_ids[0].is_empty()
    };
    if receipt.work_id != work_id || !products_match {
        return Err("Account Work receipt does not match".to_owned());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_delivery_serializes_design_mock_and_omits_absent_mock() {
        let request = LocalDeliveryRequest {
            scope_kind: "goat",
            scope_id: "scope",
            delivery_id: "delivery",
            work_id: "work",
            answer: "answer",
            products: Vec::new(),
            design_mock: Some(crate::local::DesignMock {
                media: "image/png",
                bytes: "AAEC/w==".to_owned(),
            }),
        };
        let encoded = serde_json::to_value(request).expect("design mock request should encode");
        assert_eq!(
            encoded.get("designMock"),
            Some(&serde_json::json!({
                "media": "image/png",
                "bytes": "AAEC/w=="
            }))
        );

        let absent = LocalDeliveryRequest {
            scope_kind: "goat",
            scope_id: "scope",
            delivery_id: "delivery",
            work_id: "work",
            answer: "answer",
            products: Vec::new(),
            design_mock: None,
        };
        let encoded = serde_json::to_value(absent).expect("absent mock request should encode");
        assert!(
            !encoded
                .as_object()
                .expect("delivery request should be an object")
                .contains_key("designMock")
        );
    }
}
