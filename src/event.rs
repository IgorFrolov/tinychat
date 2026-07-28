#[derive(Clone, Debug)]
pub enum ApiEvent {
    Started {
        request_id: u64,
    },
    Delta {
        request_id: u64,
        content: String,
    },
    Usage {
        request_id: u64,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        total_tokens: Option<u64>,
    },
    Finished {
        request_id: u64,
    },
    Cancelled {
        request_id: u64,
    },
    Failed {
        request_id: u64,
        message: String,
    },
}

impl ApiEvent {
    pub fn request_id(&self) -> u64 {
        match *self {
            Self::Started { request_id }
            | Self::Delta { request_id, .. }
            | Self::Usage { request_id, .. }
            | Self::Finished { request_id }
            | Self::Cancelled { request_id }
            | Self::Failed { request_id, .. } => request_id,
        }
    }
}
