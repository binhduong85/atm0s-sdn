use std::{fmt::Display, ops::Deref};

use serde::{Deserialize, Serialize};

mod behavior;
mod handler;
mod internal;
mod msg;
mod sdk;

pub(crate) const NODE_ALIAS_SERVICE_ID: u8 = 7;

pub use behavior::NodeAliasBehavior;
pub use sdk::{NodeAliasError, NodeAliasResult, NodeAliasSdk};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeAliasId(u64);

impl Display for NodeAliasId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Alias({})", self.0)
    }
}

impl From<u64> for NodeAliasId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl Deref for NodeAliasId {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    #[cfg(test)]
    mod tests {
        use crate::NodeAliasId;

        #[test]
        fn test_node_alias_id_display() {
            let alias_id = NodeAliasId(123);
            assert_eq!(format!("{}", alias_id), "Alias(123)");
        }

        #[test]
        fn test_node_alias_id_from() {
            let value: u64 = 456;
            let alias_id: NodeAliasId = value.into();
            assert_eq!(*alias_id, value);
        }
    }
}
