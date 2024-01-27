use std::path::PathBuf;
use std::{fmt::Display, str::FromStr};

use chrono::{DateTime, Utc};
use comfy_table::{
    modifiers::UTF8_ROUND_CORNERS,
    presets::{NOTHING, UTF8_FULL},
    Attribute, Cell, CellAlignment, Color, ContentArrangement, Table,
};
use crossterm::style::Stylize;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};
use uuid::Uuid;

use crate::constants::GIT_OPTION_NONE_TEXT;

#[derive(Deserialize, Serialize)]
#[typeshare::typeshare]
pub struct DeploymentInfo {
    pub id: Uuid,
    pub service_id: String,
    pub state: DeploymentState,
    pub last_update: DateTime<Utc>,
    pub git_commit_id: Option<String>,
    pub git_commit_msg: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
}

#[derive(Default, Deserialize, Serialize)]
#[typeshare::typeshare]
pub struct DeploymentRequest {
    pub data: Vec<u8>,
    pub no_test: bool,
    pub git_commit_id: Option<String>,
    pub git_commit_msg: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Display, Serialize, EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
#[strum(ascii_case_insensitive)]
#[typeshare::typeshare]
pub enum DeploymentState {
    Queued,
    Building,
    Built,
    Loading,
    Running,
    Completed,
    Stopped,
    Crashed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentMetadata {
    pub env: Environment,
    pub project_name: String,
    /// Typically your crate name
    pub service_name: String,
    /// Path to a folder that persists between deployments
    pub storage_path: PathBuf,
}

/// The environment this project is running in
#[derive(
    Clone, Copy, Debug, Default, Display, EnumString, PartialEq, Eq, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Environment {
    #[default]
    Local,
    #[strum(serialize = "production")] // Keep this around for a while for backward compat
    Deployment,
}

impl Display for DeploymentInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} deployment '{}' is {}",
            self.last_update
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string()
                .dim(),
            self.id,
            self.state
                .to_string()
                // Unwrap is safe because Color::from_str returns the color white if the argument is not a Color.
                .with(crossterm::style::Color::from_str(self.state.get_color()).unwrap())
        )
    }
}

impl DeploymentState {
    /// We return a &str rather than a Color here, since `comfy-table` re-exports
    /// crossterm::style::Color and we depend on both `comfy-table` and `crossterm`
    /// we may end up with two different versions of Color.
    pub fn get_color(&self) -> &str {
        match self {
            DeploymentState::Queued
            | DeploymentState::Building
            | DeploymentState::Built
            | DeploymentState::Loading => "cyan",
            DeploymentState::Running => "green",
            DeploymentState::Completed | DeploymentState::Stopped => "blue",
            DeploymentState::Crashed => "red",
            DeploymentState::Unknown => "yellow",
        }
    }
}

pub fn get_deployments_table(
    deployments: &Vec<DeploymentInfo>,
    service_name: &str,
    page: u32,
    raw: bool,
    page_hint: bool,
) -> String {
    if deployments.is_empty() {
        // The page starts at 1 in the CLI.
        let mut s = if page <= 1 {
            "No deployments are linked to this service\n".to_string()
        } else {
            "No more deployments are linked to this service\n".to_string()
        };
        if !raw {
            s = s.yellow().bold().to_string();
        }

        s
    } else {
        let mut table = Table::new();

        if raw {
            table
                .load_preset(NOTHING)
                .set_content_arrangement(ContentArrangement::Disabled)
                .set_header(vec![
                    Cell::new("Deployment ID").set_alignment(CellAlignment::Left),
                    Cell::new("Status").set_alignment(CellAlignment::Left),
                    Cell::new("Last updated").set_alignment(CellAlignment::Left),
                    Cell::new("Commit ID").set_alignment(CellAlignment::Left),
                    Cell::new("Commit Message").set_alignment(CellAlignment::Left),
                    Cell::new("Branch").set_alignment(CellAlignment::Left),
                    Cell::new("Dirty").set_alignment(CellAlignment::Left),
                ]);
        } else {
            table
                .load_preset(UTF8_FULL)
                .apply_modifier(UTF8_ROUND_CORNERS)
                .set_content_arrangement(ContentArrangement::DynamicFullWidth)
                .set_header(vec![
                    Cell::new("Deployment ID")
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold),
                    Cell::new("Status")
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold),
                    Cell::new("Last updated")
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold),
                    Cell::new("Commit ID")
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold),
                    Cell::new("Commit Message")
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold),
                    Cell::new("Branch")
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold),
                    Cell::new("Dirty")
                        .set_alignment(CellAlignment::Center)
                        .add_attribute(Attribute::Bold),
                ]);
        }

        for deploy in deployments.iter() {
            let truncated_commit_id = deploy
                .git_commit_id
                .as_ref()
                .map_or(String::from(GIT_OPTION_NONE_TEXT), |val| {
                    val.chars().take(7).collect()
                });

            let truncated_commit_msg = deploy
                .git_commit_msg
                .as_ref()
                .map_or(String::from(GIT_OPTION_NONE_TEXT), |val| {
                    val.chars().take(24).collect::<String>()
                });

            if raw {
                table.add_row(vec![
                    Cell::new(deploy.id),
                    Cell::new(&deploy.state),
                    Cell::new(deploy.last_update.format("%Y-%m-%dT%H:%M:%SZ")),
                    Cell::new(truncated_commit_id),
                    Cell::new(truncated_commit_msg),
                    Cell::new(
                        deploy
                            .git_branch
                            .as_ref()
                            .map_or(GIT_OPTION_NONE_TEXT, |val| val as &str),
                    ),
                    Cell::new(
                        deploy
                            .git_dirty
                            .map_or(String::from(GIT_OPTION_NONE_TEXT), |val| val.to_string()),
                    ),
                ]);
            } else {
                table.add_row(vec![
                    Cell::new(deploy.id),
                    Cell::new(&deploy.state)
                        // Unwrap is safe because Color::from_str returns the color white if str is not a Color.
                        .fg(Color::from_str(deploy.state.get_color()).unwrap())
                        .set_alignment(CellAlignment::Center),
                    Cell::new(deploy.last_update.format("%Y-%m-%dT%H:%M:%SZ"))
                        .set_alignment(CellAlignment::Center),
                    Cell::new(truncated_commit_id),
                    Cell::new(truncated_commit_msg),
                    Cell::new(
                        deploy
                            .git_branch
                            .as_ref()
                            .map_or(GIT_OPTION_NONE_TEXT, |val| val as &str),
                    ),
                    Cell::new(
                        deploy
                            .git_dirty
                            .map_or(String::from(GIT_OPTION_NONE_TEXT), |val| val.to_string()),
                    )
                    .set_alignment(CellAlignment::Center),
                ]);
            }
        }

        let formatted_table = format!("\nMost recent deployments for {service_name}\n{table}\n");
        if page_hint {
            format!(
                "{formatted_table}More deployments are available on the next page using `--page {}`\n",
                page + 1
            )
        } else {
            formatted_table
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_state_deser() {
        assert_eq!(
            DeploymentState::Queued,
            DeploymentState::from_str("Queued").unwrap()
        );
        assert_eq!(
            DeploymentState::Unknown,
            DeploymentState::from_str("unKnown").unwrap()
        );
        assert_eq!(
            DeploymentState::Built,
            DeploymentState::from_str("built").unwrap()
        );
    }

    #[test]
    fn test_env_deser() {
        assert_eq!(Environment::Local, Environment::from_str("local").unwrap());
        assert_eq!(
            Environment::Deployment,
            Environment::from_str("production").unwrap()
        );
        assert!(DeploymentState::from_str("somewhere_else").is_err());
        assert_eq!(format!("{:?}", Environment::Local), "Local".to_owned());
        assert_eq!(format!("{}", Environment::Local), "local".to_owned());
        assert_eq!(Environment::Local.to_string(), "local".to_owned());
    }
}
