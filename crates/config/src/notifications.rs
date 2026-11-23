use crate::{MAX_OBSERVATION_BATCH, valid_locale, valid_user_id};
use garde_contracts::Id;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_NOTIFICATION_RECIPIENTS: usize = 4;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationRouting {
    pub owner: NotificationOptions,
    pub private: Vec<PrivateRecipient>,
    pub forums: Vec<ForumRecipient>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NotificationOptions {
    pub locale: Option<String>,
    pub silent: bool,
    pub protect_content: bool,
    pub privacy: Option<Privacy>,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Privacy {
    Full,
    Redacted,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateRecipient {
    pub chat_id: String,
    #[serde(default)]
    pub options: NotificationOptions,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForumRecipient {
    pub chat_id: String,
    pub default_topic_id: ForumTopic,
    #[serde(default)]
    pub topics: Vec<ForumRoute>,
    #[serde(default)]
    pub options: NotificationOptions,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForumRoute {
    pub event_type: Id,
    pub topic_id: ForumTopic,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ForumTopic {
    General(()),
    Thread(i32),
}

impl ForumTopic {
    pub fn thread_id(self) -> Option<i32> {
        match self {
            Self::General(()) => None,
            Self::Thread(id) => Some(id),
        }
    }

    fn valid(self) -> bool {
        // Telegram rejects the General topic ID (1) as an explicit sendMessage thread.
        self.thread_id().is_none_or(|id| id > 1)
    }
}

pub fn valid_notification_destination(chat_id: &str, topic_id: Option<i32>) -> bool {
    match chat_id.strip_prefix('-') {
        Some(id) => valid_user_id(id) && topic_id.is_none_or(|id| id > 1),
        None => topic_id.is_none() && valid_user_id(chat_id),
    }
}

impl NotificationOptions {
    fn valid(&self) -> bool {
        self.locale.as_deref().is_none_or(valid_locale)
    }
}

impl NotificationRouting {
    pub(crate) fn valid(&self, owner: Option<&str>, pending_deliveries: u32) -> bool {
        let recipients = 1 + self.private.len() + self.forums.len();
        if recipients > MAX_NOTIFICATION_RECIPIENTS
            || recipients * MAX_OBSERVATION_BATCH > pending_deliveries as usize
            || !self.owner.valid()
        {
            return false;
        }
        let mut chats = HashSet::new();
        chats.extend(owner);
        self.private.iter().all(|recipient| {
            valid_user_id(&recipient.chat_id)
                && chats.insert(recipient.chat_id.as_str())
                && recipient.options.valid()
        }) && self.forums.iter().all(|forum| {
            let mut routes = HashSet::new();
            forum.chat_id.strip_prefix('-').is_some_and(valid_user_id)
                && forum.default_topic_id.valid()
                && chats.insert(forum.chat_id.as_str())
                && forum.options.valid()
                && forum.topics.len() <= 64
                && forum
                    .topics
                    .iter()
                    .all(|route| route.topic_id.valid() && routes.insert(route.event_type.as_str()))
        })
    }
}
