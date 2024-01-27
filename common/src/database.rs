use std::{fmt::Display, str::FromStr};

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseType {
    AwsRds(AwsRdsEngine),
    Shared(SharedEngine),
}

#[derive(Clone, Copy, Debug, Deserialize, Display, Serialize, EnumString, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum AwsRdsEngine {
    Postgres,
    MySql,
    MariaDB,
}

#[derive(Clone, Copy, Debug, Deserialize, Display, Serialize, EnumString, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SharedEngine {
    Postgres,
    MongoDb,
}

impl Display for DatabaseType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DatabaseType::AwsRds(rds_type) => write!(f, "aws_rds::{rds_type}"),
            DatabaseType::Shared(shared_type) => write!(f, "shared::{shared_type}"),
        }
    }
}

impl FromStr for DatabaseType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if let Some((prefix, rest)) = s.split_once("::") {
            match prefix {
                "aws_rds" => Ok(Self::AwsRds(
                    AwsRdsEngine::from_str(rest).map_err(|e| e.to_string())?,
                )),
                "shared" => Ok(Self::Shared(
                    SharedEngine::from_str(rest).map_err(|e| e.to_string())?,
                )),
                _ => Err(format!("'{prefix}' is an unknown database type")),
            }
        } else {
            Err(format!("'{s}' is an unknown database type"))
        }
    }
}
