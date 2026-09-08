use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The categories of network/media content that can be blocked in VRChat.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BlockCategory {
    Video,
    Images,
    Strings,
    Custom(String),
    Rest,
}

impl BlockCategory {
    pub fn label(&self) -> &str {
        match self {
            Self::Video => "Videos",
            Self::Images => "Images",
            Self::Strings => "Strings",
            Self::Custom(name) => name.as_str(),
            Self::Rest => "Rest",
        }
    }

    pub fn short_label(&self) -> &str {
        match self {
            Self::Video => "Video",
            Self::Images => "Image",
            Self::Strings => "String",
            Self::Custom(name) => name.as_str(),
            Self::Rest => "Rest",
        }
    }

    pub fn header_tag(&self) -> String {
        match self {
            Self::Video => "# ----- BEGIN LVR VIDEO BLOCK -----".to_string(),
            Self::Images => "# ----- BEGIN LVR IMAGE BLOCK -----".to_string(),
            Self::Strings => "# ----- BEGIN LVR STRING BLOCK -----".to_string(),
            Self::Custom(name) => format!("# ----- BEGIN LVR {} BLOCK -----", name.to_uppercase()),
            Self::Rest => "# ----- BEGIN LVR REST BLOCK -----".to_string(),
        }
    }

    pub fn footer_tag(&self) -> String {
        match self {
            Self::Video => "# ----- END LVR VIDEO BLOCK -----".to_string(),
            Self::Images => "# ----- END LVR IMAGE BLOCK -----".to_string(),
            Self::Strings => "# ----- END LVR STRING BLOCK -----".to_string(),
            Self::Custom(name) => format!("# ----- END LVR {} BLOCK -----", name.to_uppercase()),
            Self::Rest => "# ----- END LVR REST BLOCK -----".to_string(),
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        let trimmed = tag.trim();
        let middle = trimmed
            .strip_prefix("# ----- BEGIN LVR ")?
            .strip_suffix(" BLOCK -----")?
            .trim();
        match middle {
            "VIDEO" => Some(Self::Video),
            "IMAGE" => Some(Self::Images),
            "STRING" => Some(Self::Strings),
            "REST" => Some(Self::Rest),
            custom => {
                let lists = crate::domain_block::active_domains();
                if let Some(matching) = lists
                    .custom_categories
                    .iter()
                    .find(|c| c.eq_ignore_ascii_case(custom))
                {
                    Some(Self::Custom(matching.clone()))
                } else {
                    Some(Self::Custom(custom.to_string()))
                }
            }
        }
    }
}

/// Detailed domain counts per category.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategoryCounts {
    pub video: usize,
    pub image: usize,
    pub string: usize,
    #[serde(default)]
    pub custom: BTreeMap<String, usize>,
    pub rest: usize,
}

impl CategoryCounts {
    pub fn total(&self) -> usize {
        self.video + self.image + self.string + self.custom.values().sum::<usize>() + self.rest
    }

    pub fn for_category(&self, cat: &BlockCategory) -> usize {
        match cat {
            BlockCategory::Video => self.video,
            BlockCategory::Images => self.image,
            BlockCategory::Strings => self.string,
            BlockCategory::Rest => self.rest,
            BlockCategory::Custom(name) => self
                .custom
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, &c)| c)
                .unwrap_or(0),
        }
    }

    pub fn set_for_category(&mut self, cat: &BlockCategory, count: usize) {
        match cat {
            BlockCategory::Video => self.video = count,
            BlockCategory::Images => self.image = count,
            BlockCategory::Strings => self.string = count,
            BlockCategory::Rest => self.rest = count,
            BlockCategory::Custom(name) => {
                if let Some(existing_key) = self
                    .custom
                    .keys()
                    .find(|k| k.eq_ignore_ascii_case(name))
                    .cloned()
                {
                    self.custom.insert(existing_key, count);
                } else {
                    self.custom.insert(name.clone(), count);
                }
            }
        }
    }
}

/// Statistics for a specific blocklist source (Official or Community).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlocklistStats {
    pub name: String,
    pub counts: CategoryCounts,
    pub enabled: bool,
}

/// Raw JSON blocklist payload with metadata.
#[derive(Debug, Clone)]
pub struct RawBlocklistInput {
    pub name: String,
    pub json: String,
    pub enabled: bool,
}

/// Dynamic or fallback domain store for all categories.
#[derive(Debug, Clone)]
pub struct DomainLists {
    pub video_domains: Vec<String>,
    pub image_domains: Vec<String>,
    pub string_domains: Vec<String>,
    pub custom_domains: BTreeMap<String, Vec<String>>,
    pub rest_domains: Vec<String>,
    #[cfg(test)]
    pub protected_domains: Vec<String>,
    pub list_stats: Vec<BlocklistStats>,
    pub total_counts: CategoryCounts,
    pub custom_categories: Vec<String>,
}

impl DomainLists {
    pub fn all_categories(&self) -> Vec<BlockCategory> {
        let mut cats = vec![
            BlockCategory::Video,
            BlockCategory::Images,
            BlockCategory::Strings,
        ];
        for custom in &self.custom_categories {
            cats.push(BlockCategory::Custom(custom.clone()));
        }
        cats.push(BlockCategory::Rest);
        cats
    }

    pub fn domains_for_category(&self, cat: &BlockCategory) -> &[String] {
        match cat {
            BlockCategory::Video => &self.video_domains,
            BlockCategory::Images => &self.image_domains,
            BlockCategory::Strings => &self.string_domains,
            BlockCategory::Rest => &self.rest_domains,
            BlockCategory::Custom(name) => self
                .custom_domains
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_slice())
                .unwrap_or(&[]),
        }
    }
}

/// State of all domain blocking categories.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockState {
    pub video_blocked: bool,
    pub images_blocked: bool,
    pub strings_blocked: bool,
    pub rest_blocked: bool,
    #[serde(default)]
    pub custom_blocked: BTreeMap<String, bool>,
}

impl BlockState {
    pub fn is_blocked(&self, category: &BlockCategory) -> bool {
        match category {
            BlockCategory::Video => self.video_blocked,
            BlockCategory::Images => self.images_blocked,
            BlockCategory::Strings => self.strings_blocked,
            BlockCategory::Rest => self.rest_blocked,
            BlockCategory::Custom(name) => self
                .custom_blocked
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, &b)| b)
                .unwrap_or(false),
        }
    }

    pub fn set_blocked(&mut self, category: &BlockCategory, blocked: bool) {
        match category {
            BlockCategory::Video => self.video_blocked = blocked,
            BlockCategory::Images => self.images_blocked = blocked,
            BlockCategory::Strings => self.strings_blocked = blocked,
            BlockCategory::Rest => self.rest_blocked = blocked,
            BlockCategory::Custom(name) => {
                if let Some(existing_key) = self
                    .custom_blocked
                    .keys()
                    .find(|k| k.eq_ignore_ascii_case(name))
                    .cloned()
                {
                    self.custom_blocked.insert(existing_key, blocked);
                } else {
                    self.custom_blocked.insert(name.clone(), blocked);
                }
            }
        }
    }
}
