use std::{fmt::Display, str::FromStr};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::database::DatabaseType;

/// Common type to hold all the information we need for a generic resource
#[derive(Clone, Deserialize, Serialize)]
pub struct ResourceInfo {
    /// The type of this resource.
    pub r#type: ResourceType,

    /// The config used when creating this resource. Use the [Self::r#type] to know how to parse this data.
    pub config: Value,

    /// The data associated with this resource. Use the [Self::r#type] to know how to parse this data.
    pub data: Value,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceType {
    Database(DatabaseType),
    Secrets,
    StaticFolder,
    Persist,
    Turso,
    Metadata,
    Custom,
}

impl FromStr for ResourceType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some((prefix, rest)) = s.split_once("::") {
            match prefix {
                "database" => Ok(Self::Database(DatabaseType::from_str(rest)?)),
                _ => Err(format!("'{prefix}' is an unknown resource type")),
            }
        } else {
            match s {
                "secrets" => Ok(Self::Secrets),
                "static_folder" => Ok(Self::StaticFolder),
                "metadata" => Ok(Self::Metadata),
                "persist" => Ok(Self::Persist),
                "turso" => Ok(Self::Turso),
                "custom" => Ok(Self::Custom),
                _ => Err(format!("'{s}' is an unknown resource type")),
            }
        }
    }
}

impl ResourceInfo {
    pub fn into_bytes(self) -> Vec<u8> {
        self.to_bytes()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("to turn resource into a vec")
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        serde_json::from_slice(&bytes).expect("to turn bytes into a resource")
    }
}

impl Display for ResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceType::Database(db_type) => write!(f, "database::{db_type}"),
            ResourceType::Secrets => write!(f, "secrets"),
            ResourceType::StaticFolder => write!(f, "static_folder"),
            ResourceType::Persist => write!(f, "persist"),
            ResourceType::Turso => write!(f, "turso"),
            ResourceType::Metadata => write!(f, "metadata"),
            ResourceType::Custom => write!(f, "custom"),
        }
    }
}
