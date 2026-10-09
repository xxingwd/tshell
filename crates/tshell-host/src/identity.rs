use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostId(String);
impl Default for HostId {
    fn default() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}
impl HostId {
    pub fn local() -> Self {
        Self("local".into())
    }
    pub fn is_remote(&self) -> bool {
        uuid::Uuid::parse_str(&self.0).is_ok_and(|id| !id.is_nil())
    }
}
impl fmt::Display for HostId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}
