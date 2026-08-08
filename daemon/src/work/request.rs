//! Work requests preserve exact user meaning, ordered context, and authority.

// Serialization reads the Browser request once; execution receives the original fields directly.
use serde::{Deserialize, Serialize};

/// Provenance prevents external context from inheriting user authority.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provenance {
    User,
    Agent,
    External,
}

/// WorkContext preserves one ordered message and its trust boundary.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkContext {
    author: String,
    source: String,
    provenance: Provenance,
    text: String,
    #[serde(default)]
    attachments: Vec<String>,
}

impl WorkContext {
    /// DesignerGuidance recognizes only the Account-authored instruction row.
    fn designer_guidance(&self) -> bool {
        self.author == "TrianGoat Designer"
            && self.source == "guidance"
            && matches!(self.provenance, Provenance::Agent)
    }
}

/// WorkRequest transports complete browser evidence without semantic filtering.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkRequest {
    #[serde(default)]
    pub(super) conversation: String,
    // The user's original goal remains the primary execution text.
    pub(super) goal: String,
    #[serde(default)]
    // Ordered context preserves author, source, provenance, text, and attachment references.
    pub(super) context: Vec<WorkContext>,
}

// WorkRequest keeps exact new and follow-up user evidence.
impl WorkRequest {
    pub(crate) fn conversation_id(&self, work_id: &str) -> Result<String, String> {
        let value = (!self.conversation.is_empty())
            .then_some(self.conversation.as_str())
            .unwrap_or(work_id);
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err("Conversation identity is invalid".to_owned());
        }
        Ok(value.to_owned())
    }

    /// Frozen steering carries only the next user text; it joins the original native session unchanged.
    pub(crate) fn follow_up(goal: String) -> Self {
        Self {
            conversation: String::new(),
            goal,
            context: Vec::new(),
        }
    }

    /// TakeDesignerGuidance lifts the one Account instruction while all remaining context stays data.
    pub(super) fn take_designer_guidance(&mut self) -> Result<Option<String>, String> {
        let mut matches = self
            .context
            .iter()
            .filter(|entry| entry.designer_guidance());
        let guidance = matches.next().map(|entry| entry.text.clone());
        if matches.next().is_some() {
            return Err("Work contains multiple Designer guidance entries".to_owned());
        }
        self.context.retain(|entry| !entry.designer_guidance());
        Ok(guidance)
    }
}
