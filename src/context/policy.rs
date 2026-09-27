use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Lead,
    Architect,
    Worker,
    Reviewer,
    Verifier,
}

impl Role {
    pub const fn max_input_tokens(self) -> usize {
        match self {
            Self::Lead => 12_000,
            Self::Architect | Self::Reviewer => 20_000,
            Self::Worker => 30_000,
            Self::Verifier => 8_000,
        }
    }

    const fn accepts(self, kind: ContextKind) -> bool {
        match self {
            Self::Lead => matches!(
                kind,
                ContextKind::Task | ContextKind::RepositoryMap | ContextKind::SkillMetadata
            ),
            Self::Architect => matches!(
                kind,
                ContextKind::Task | ContextKind::RepositoryMap | ContextKind::Decision
            ),
            Self::Worker => matches!(
                kind,
                ContextKind::Task
                    | ContextKind::Decision
                    | ContextKind::Skill
                    | ContextKind::Source
            ),
            Self::Reviewer => matches!(
                kind,
                ContextKind::Task | ContextKind::Patch | ContextKind::Check | ContextKind::Source
            ),
            Self::Verifier => matches!(kind, ContextKind::Task | ContextKind::Check),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Task,
    RepositoryMap,
    Decision,
    SkillMetadata,
    Skill,
    Source,
    Patch,
    Check,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextPart {
    pub kind: ContextKind,
    pub content: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SnapshotItem {
    pub kind: ContextKind,
    pub bytes: usize,
    pub estimated_tokens: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ContextSnapshot {
    pub role: Role,
    pub items: Vec<SnapshotItem>,
    pub bytes: usize,
    pub estimated_tokens: usize,
    pub trimmed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedContext {
    pub parts: Vec<ContextPart>,
    pub snapshot: ContextSnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyError {
    InvalidLimit,
    SplitRequired,
}

/// Memilih context menurut role; hanya excerpt source yang boleh dipangkas.
/// ponytail: kategori artifact harus ditetapkan dari provenance tepercaya oleh pemanggil; verifikasi metadata saat wiring store.
pub fn apply(
    role: Role,
    parts: impl IntoIterator<Item = ContextPart>,
    task_limit: usize,
) -> Result<PreparedContext, PolicyError> {
    if task_limit == 0 {
        return Err(PolicyError::InvalidLimit);
    }
    let limit = task_limit.min(role.max_input_tokens());
    let mut accepted: Vec<_> = parts
        .into_iter()
        .filter(|part| role.accepts(part.kind))
        .collect();
    let mut remaining = limit;
    let mut trimmed = false;
    let mut selected = Vec::new();
    // Materi wajib diproses sebelum source agar excerpt tidak memakan jatah kontrak/diff.
    for source_pass in [false, true] {
        for part in &mut accepted {
            if (part.kind == ContextKind::Source) != source_pass {
                continue;
            }
            let tokens = part.content.len().div_ceil(4);
            if tokens > remaining {
                if !source_pass {
                    return Err(PolicyError::SplitRequired);
                }
                let mut end = remaining.saturating_mul(4).min(part.content.len());
                while !part.content.is_char_boundary(end) {
                    end -= 1;
                }
                part.content.truncate(end);
                trimmed = true;
            }
            if part.content.is_empty() && source_pass {
                continue;
            }
            remaining -= part.content.len().div_ceil(4);
            selected.push(part.clone());
        }
    }
    let items: Vec<_> = selected
        .iter()
        .map(|part| SnapshotItem {
            kind: part.kind,
            bytes: part.content.len(),
            estimated_tokens: part.content.len().div_ceil(4),
        })
        .collect();
    Ok(PreparedContext {
        parts: selected,
        snapshot: ContextSnapshot {
            role,
            bytes: items.iter().map(|item| item.bytes).sum(),
            estimated_tokens: limit - remaining,
            items,
            trimmed,
        },
    })
}
