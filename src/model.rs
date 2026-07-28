use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    User,
    Assistant,
    System,
}

impl Role {
    pub fn api_name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MessageState {
    Complete,
    Streaming,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub id: u64,
    pub role: Role,
    pub content: String,
    pub state: MessageState,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TokenUsage {
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Eq, PartialEq)]
pub struct RequestMessage {
    pub role: &'static str,
    pub content: String,
}

pub fn messages_for_request(messages: &[Message], system_prompt: &str) -> Vec<RequestMessage> {
    let mut result = Vec::with_capacity(messages.len() + usize::from(!system_prompt.is_empty()));

    if !system_prompt.is_empty() {
        let system_role = Role::System;
        result.push(RequestMessage {
            role: system_role.api_name(),
            content: system_prompt.to_owned(),
        });
    }

    result.extend(
        messages
            .iter()
            .filter(|message| {
                !message.content.trim().is_empty()
                    && match message.role {
                        Role::User => true,
                        Role::Assistant | Role::System => message.state == MessageState::Complete,
                    }
            })
            .map(|message| RequestMessage {
                role: message.role.api_name(),
                content: message.content.clone(),
            }),
    );

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_messages_for_next_request() {
        let messages = vec![
            Message {
                id: 1,
                role: Role::User,
                content: "hello".into(),
                state: MessageState::Complete,
            },
            Message {
                id: 2,
                role: Role::Assistant,
                content: "complete".into(),
                state: MessageState::Complete,
            },
            Message {
                id: 3,
                role: Role::Assistant,
                content: "partial".into(),
                state: MessageState::Cancelled,
            },
            Message {
                id: 4,
                role: Role::Assistant,
                content: "local error".into(),
                state: MessageState::Failed,
            },
            Message {
                id: 5,
                role: Role::Assistant,
                content: String::new(),
                state: MessageState::Streaming,
            },
        ];

        let result = messages_for_request(&messages, "be helpful");
        assert_eq!(
            result,
            vec![
                RequestMessage {
                    role: "system",
                    content: "be helpful".into()
                },
                RequestMessage {
                    role: "user",
                    content: "hello".into()
                },
                RequestMessage {
                    role: "assistant",
                    content: "complete".into()
                }
            ]
        );
    }
}
