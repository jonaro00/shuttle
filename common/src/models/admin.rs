use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub struct ProjectAccountPair {
    pub project_name: String,
    pub account_name: String,
}
